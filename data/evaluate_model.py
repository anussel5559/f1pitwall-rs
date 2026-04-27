#!/usr/bin/env python3
"""Evaluate pit window predictions against actual race data across multiple races."""

import sqlite3

DB = "/Users/alexnussel/Library/Application Support/f1-pitwall/f1-pitwall.db"
FUEL_EFFECT = 0.06  # s/lap
DELTA_THRESHOLD = 2.0

# Compound-specific default lives
COMPOUND_LIFE = {"SOFT": 18, "MEDIUM": 26, "HARD": 35}
DEFAULT_LIFE = 35

RACES = [
    {
        "name": "Melbourne 2026",
        "session_key": 11234,
        "total_laps": 58,
        # Exclude stints ending during SC/VSC windows
        "sc_windows": [(11, 14), (17, 20), (33, 35)],
    },
    {
        "name": "Shanghai 2026",
        "session_key": 11245,
        "total_laps": 56,
        "sc_windows": [(8, 13)],
    },
    {
        "name": "Suzuka 2026",
        "session_key": 11253,
        "total_laps": 53,
        "sc_windows": [(21, 27)],
    },
]

db = sqlite3.connect(DB)


def evaluate_race(race):
    sk = race["session_key"]
    total_laps = race["total_laps"]
    sc_windows = race["sc_windows"]

    # Build SC exclusion clause
    sc_clauses = " ".join(
        f"AND NOT (s.lap_end BETWEEN {lo} AND {hi})" for lo, hi in sc_windows
    )

    # All completed stints for field evidence (including short ones for averaging)
    all_stints = db.execute(f"""
        SELECT s.driver_number, s.stint_number, s.compound,
               s.lap_start, s.lap_end,
               COALESCE(s.tyre_age_at_start, 0) as tyre_age_start,
               s.lap_end - s.lap_start + 1 as stint_laps
        FROM stints s
        WHERE s.session_key=?
          AND s.lap_end IS NOT NULL
          AND s.lap_end < {total_laps - 2}
          {sc_clauses}
          AND (s.lap_end - s.lap_start + 1) >= 5
        ORDER BY s.lap_end
    """, (sk,)).fetchall()

    stints = db.execute(f"""
        SELECT s.driver_number, d.name_acronym, s.stint_number, s.compound,
               s.lap_start, s.lap_end, s.lap_end - s.lap_start + 1
        FROM stints s
        JOIN drivers d ON d.session_key=s.session_key AND d.driver_number=s.driver_number
        WHERE s.session_key=?
          AND s.lap_end IS NOT NULL AND s.lap_end < {total_laps}
          {sc_clauses}
          AND (s.lap_end - s.lap_start + 1) >= 10
        ORDER BY s.lap_end
    """, (sk,)).fetchall()

    results = []

    for dn, acr, stint_num, compound, lap_start, actual_pit, stint_laps in stints:
        is_final = actual_pit >= total_laps - 2

        laps = db.execute("""
            SELECT l.lap_number, l.lap_duration
            FROM laps l
            JOIN stints s ON s.session_key=l.session_key
                AND s.driver_number=l.driver_number
                AND l.lap_number >= s.lap_start
                AND l.lap_number <= COALESCE(s.lap_end, 999)
            WHERE l.session_key=? AND l.driver_number=? AND s.stint_number=?
              AND l.lap_duration IS NOT NULL AND l.lap_duration > 0
              AND l.is_pit_out_lap = 0
              AND (s.lap_end IS NULL OR l.lap_number != s.lap_end)
              AND l.lap_duration < 120
            ORDER BY l.lap_number
        """, (sk, dn, stint_num)).fetchall()

        if len(laps) < 8:
            continue

        compound_life = COMPOUND_LIFE.get(compound or "", DEFAULT_LIFE)

        for cp in [8, 12, 16, 20]:
            if cp > len(laps):
                break

            current_lap = laps[cp - 1][0]

            # Field evidence: completed stints on same compound that finished
            # BEFORE this checkpoint (can't use future data)
            field_lengths = [
                s[6]  # stint_laps
                for s in all_stints
                if s[2] == compound  # same compound
                and s[4] < current_lap  # completed before this checkpoint
                and s[0] != dn  # not the same driver
            ]
            field_avg_life = None
            if len(field_lengths) >= 2:
                field_avg_life = sum(field_lengths) // len(field_lengths)

            subset = laps[:cp]
            n = len(subset)
            ages = [l[0] - lap_start for l in subset]
            corrected = [l[1] - FUEL_EFFECT * (total_laps - l[0]) for l in subset]

            sx = sum(ages)
            sy = sum(corrected)
            sxy = sum(a * t for a, t in zip(ages, corrected))
            sxx = sum(a * a for a in ages)
            denom = n * sxx - sx * sx
            if denom == 0:
                continue
            slope = (n * sxy - sx * sy) / denom

            # Start with compound default, then apply tighter bounds
            expiry_age = compound_life

            # Bound 1: field completed stint evidence (strongest signal)
            if field_avg_life is not None and field_avg_life < expiry_age:
                expiry_age = field_avg_life

            # Bound 2: deg rate threshold
            if slope > 0.001:
                threshold_age = int(DELTA_THRESHOLD / slope)
                if threshold_age > 0 and threshold_age < expiry_age:
                    expiry_age = threshold_age

            current_age = subset[-1][0] - lap_start
            current_lap = subset[-1][0]
            laps_remaining = max(expiry_age - current_age, 0)
            predicted_pit = current_lap + laps_remaining
            error = predicted_pit - actual_pit

            results.append({
                "race": race["name"],
                "driver": acr,
                "compound": compound or "?",
                "stint": stint_num,
                "actual_pit": actual_pit,
                "is_final": is_final,
                "checkpoint_laps": cp,
                "checkpoint_at_lap": current_lap,
                "fc_deg_rate": round(slope, 4),
                "expiry_age": expiry_age,
                "field_evidence": f"{field_avg_life}L ({len(field_lengths)})" if field_avg_life else "-",
                "current_age": current_age,
                "predicted_pit": predicted_pit,
                "error": error,
            })

    return results


def print_stats(label, subset):
    if not subset:
        print(f"\n=== {label}: no data ===")
        return
    errors = [r["error"] for r in subset]
    abs_errors = [abs(e) for e in errors]
    n = len(errors)
    stint_count = len(set((r["race"], r["driver"], r["stint"]) for r in subset))

    print(f"\n=== {label} ({n} predictions across {stint_count} stints) ===")
    print(f"Mean error (bias): {sum(errors)/n:+.1f} laps")
    print(f"Mean abs error:    {sum(abs_errors)/n:.1f} laps")
    print(f"Median abs error:  {sorted(abs_errors)[n//2]:.0f} laps")
    print(f"Within  3 laps:    {sum(1 for e in abs_errors if e <= 3):>2}/{n} ({100*sum(1 for e in abs_errors if e <= 3)/n:.0f}%)")
    print(f"Within  5 laps:    {sum(1 for e in abs_errors if e <= 5):>2}/{n} ({100*sum(1 for e in abs_errors if e <= 5)/n:.0f}%)")
    print(f"Within 10 laps:    {sum(1 for e in abs_errors if e <= 10):>2}/{n} ({100*sum(1 for e in abs_errors if e <= 10)/n:.0f}%)")

    print(f"\n  By checkpoint:")
    for cp_val in [8, 12, 16, 20]:
        cp_results = [r for r in subset if r["checkpoint_laps"] == cp_val]
        if not cp_results:
            continue
        cp_errors = [abs(r["error"]) for r in cp_results]
        cp_bias = [r["error"] for r in cp_results]
        cn = len(cp_errors)
        print(
            f"  At {cp_val:>2} laps: n={cn:>2}, "
            f"MAE={sum(cp_errors)/cn:.1f}, "
            f"bias={sum(cp_bias)/cn:+.1f}, "
            f"within 5={sum(1 for e in cp_errors if e<=5)}/{cn}"
        )

    print(f"\n  By compound:")
    for cmpd in ["SOFT", "MEDIUM", "HARD"]:
        c_results = [r for r in subset if r["compound"] == cmpd]
        if not c_results:
            continue
        c_errors = [abs(r["error"]) for r in c_results]
        c_bias = [r["error"] for r in c_results]
        cn = len(c_errors)
        print(
            f"  {cmpd:<8}: n={cn:>2}, "
            f"MAE={sum(c_errors)/cn:.1f}, "
            f"bias={sum(c_bias)/cn:+.1f}"
        )


# Run all races
all_results = []
for race in RACES:
    race_results = evaluate_race(race)
    all_results.extend(race_results)

    # Per-race detail
    genuine = [r for r in race_results if not r["is_final"]]
    final = [r for r in race_results if r["is_final"]]

    print(f"\n{'='*70}")
    print(f"  {race['name']} (session {race['session_key']}, {race['total_laps']} laps)")
    print(f"{'='*70}")

    header = f"{'Drv':<4} {'Cmpd':<7} {'St':>2} {'ActPit':>6} {'Final':>5} {'@Lap':>5} {'Clnx':>4} {'FC_Deg':>7} {'Field':>9} {'ExpAge':>6} {'Pred':>5} {'Err':>5}"
    print(header)
    print("-" * len(header))
    for r in race_results:
        sign = "+" if r["error"] >= 0 else ""
        fin = "  *" if r["is_final"] else ""
        print(
            f"{r['driver']:<4} {r['compound']:<7} {r['stint']:>2} "
            f"{r['actual_pit']:>6}{fin:>5} {r['checkpoint_at_lap']:>5} {r['checkpoint_laps']:>4} "
            f"{r['fc_deg_rate']:>7.4f} {r['field_evidence']:>9} {r['expiry_age']:>6} "
            f"{r['predicted_pit']:>5} {sign}{r['error']:>4}"
        )

    if genuine:
        print_stats(f"{race['name']} — Genuine pit stops", genuine)
    if final:
        print_stats(f"{race['name']} — Final stints (ran to finish)", final)

# Cross-race summary
print(f"\n{'='*70}")
print(f"  CROSS-RACE SUMMARY")
print(f"{'='*70}")

all_genuine = [r for r in all_results if not r["is_final"]]
all_final = [r for r in all_results if r["is_final"]]

print_stats("ALL RACES — Genuine pit stops", all_genuine)
print_stats("ALL RACES — Final stints", all_final)
print_stats("ALL RACES — Combined", all_results)

# Per-race comparison
print(f"\n  Per-race genuine pit stop summary:")
print(f"  {'Race':<20} {'n':>4} {'MAE':>6} {'Bias':>7} {'W/in5':>7}")
print(f"  {'-'*46}")
for race in RACES:
    genuine = [r for r in all_results if r["race"] == race["name"] and not r["is_final"]]
    if not genuine:
        continue
    errors = [r["error"] for r in genuine]
    abs_errors = [abs(e) for e in errors]
    n = len(errors)
    w5 = sum(1 for e in abs_errors if e <= 5)
    print(
        f"  {race['name']:<20} {n:>4} {sum(abs_errors)/n:>6.1f} {sum(errors)/n:>+7.1f} {w5:>3}/{n}"
    )
