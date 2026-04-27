#!/usr/bin/env node
/**
 * Update data/tracks/{key}.json records from a completed season.
 *
 * For the given --year, walks every Grand Prix meeting via OpenF1:
 *   - fastest green-flag race lap        -> race_lap_record (only if faster than stored)
 *   - fastest qualifying lap (any of Q1/Q2/Q3)
 *                                        -> qualifying_record (only if faster than stored)
 *   - race winner                        -> previous_winner (always overwritten with the given year)
 *
 * Sprint sessions are ignored for record-setting (FIA race lap record is
 * Race-only). Pit-in and pit-out laps are filtered out before comparing.
 *
 * Existing hand-curated record objects are preserved when the current-year
 * values don't beat them. Only fields with fresher data get written.
 *
 * Usage:
 *   node scripts/update-track-records.mjs --year 2025
 *   node scripts/update-track-records.mjs --year 2025 --circuit bahrain
 *   node scripts/update-track-records.mjs --year 2025 --dry-run
 */

import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..');
const TRACKS_DIR = path.join(REPO_ROOT, 'data', 'tracks');

// Must stay in sync with scripts/fetch-tracks.mjs and resolve_circuit() in
// crates/f1core/src/domain/track.rs.
const NAME_MAP = {
	Sakhir: 'bahrain',
	Jeddah: 'jeddah',
	Melbourne: 'albert_park',
	Suzuka: 'suzuka',
	Shanghai: 'shanghai',
	Miami: 'miami',
	Imola: 'imola',
	'Monte Carlo': 'monaco',
	Montreal: 'montreal',
	Catalunya: 'barcelona',
	Spielberg: 'spielberg',
	Silverstone: 'silverstone',
	Hungaroring: 'hungaroring',
	'Spa-Francorchamps': 'spa',
	Zandvoort: 'zandvoort',
	Monza: 'monza',
	Madring: 'madring',
	Baku: 'baku',
	Singapore: 'marina_bay',
	Austin: 'austin',
	'Mexico City': 'mexico',
	Interlagos: 'interlagos',
	'Las Vegas': 'las_vegas',
	Lusail: 'lusail',
	'Yas Marina Circuit': 'yas_marina',
};

function parseArgs(argv) {
	const args = { year: null, circuit: null, dryRun: false };
	for (let i = 0; i < argv.length; i++) {
		const a = argv[i];
		if (a === '--year') args.year = Number(argv[++i]);
		else if (a === '--circuit') args.circuit = argv[++i];
		else if (a === '--dry-run') args.dryRun = true;
	}
	if (!args.year || Number.isNaN(args.year)) {
		console.error('Usage: update-track-records.mjs --year YYYY [--circuit key] [--dry-run]');
		process.exit(1);
	}
	return args;
}

async function fetchJson(url) {
	const res = await fetch(url);
	if (!res.ok) throw new Error(`${url} -> ${res.status}`);
	return res.json();
}

async function loadTrack(key) {
	const file = path.join(TRACKS_DIR, `${key}.json`);
	const raw = await fs.readFile(file, 'utf-8');
	return { file, data: JSON.parse(raw) };
}

async function writeTrack(file, data) {
	await fs.writeFile(file, JSON.stringify(data, null, 2) + '\n');
}

/**
 * Return the fastest lap entry `{ lap_duration, driver_number }` from a list,
 * filtering pit-in/out laps and null durations. Returns null if none.
 */
function fastestLap(laps) {
	let best = null;
	for (const lap of laps) {
		if (lap.is_pit_out_lap) continue;
		if (lap.duration_sector_1 == null) continue; // pit-in laps have nulls in sectors
		const t = lap.lap_duration;
		if (t == null) continue;
		if (!best || t < best.lap_duration) best = lap;
	}
	return best;
}

function teamForDriver(drivers, driverNumber) {
	const d = drivers.find((x) => x.driver_number === driverNumber);
	return d?.team_name ?? 'Unknown';
}

function driverName(drivers, driverNumber) {
	const d = drivers.find((x) => x.driver_number === driverNumber);
	if (!d) return `#${driverNumber}`;
	return d.full_name ?? d.broadcast_name ?? `#${driverNumber}`;
}

async function processMeeting(meeting, year, existing) {
	const key = NAME_MAP[meeting.circuit_short_name];
	if (!key) {
		return { key: null, skip: `unknown circuit "${meeting.circuit_short_name}"` };
	}
	const sessions = await fetchJson(
		`https://api.openf1.org/v1/sessions?meeting_key=${meeting.meeting_key}`,
	);
	const raceSession = sessions.find(
		(s) => s.session_type === 'Race' && s.session_name === 'Race',
	);
	const qualifyingSession = sessions.find(
		(s) => s.session_type === 'Qualifying' && s.session_name === 'Qualifying',
	);

	const updates = { key };

	if (qualifyingSession) {
		const [laps, drivers] = await Promise.all([
			fetchJson(`https://api.openf1.org/v1/laps?session_key=${qualifyingSession.session_key}`),
			fetchJson(`https://api.openf1.org/v1/drivers?session_key=${qualifyingSession.session_key}`),
		]);
		const best = fastestLap(laps);
		if (best) {
			updates.qualifying = {
				time_s: Number(best.lap_duration.toFixed(3)),
				driver: driverName(drivers, best.driver_number),
				team: teamForDriver(drivers, best.driver_number),
				year,
			};
		}
	}

	if (raceSession) {
		const [laps, drivers, results] = await Promise.all([
			fetchJson(`https://api.openf1.org/v1/laps?session_key=${raceSession.session_key}`),
			fetchJson(`https://api.openf1.org/v1/drivers?session_key=${raceSession.session_key}`),
			fetchJson(`https://api.openf1.org/v1/session_result?session_key=${raceSession.session_key}`),
		]);
		const best = fastestLap(laps);
		if (best) {
			updates.raceLap = {
				time_s: Number(best.lap_duration.toFixed(3)),
				driver: driverName(drivers, best.driver_number),
				team: teamForDriver(drivers, best.driver_number),
				year,
			};
		}
		const winnerEntry = results.find((r) => r.position === 1);
		if (winnerEntry) {
			updates.winner = {
				year,
				driver: driverName(drivers, winnerEntry.driver_number),
				team: teamForDriver(drivers, winnerEntry.driver_number),
			};
		}
	}

	return updates;
}

function mergeRecords(existing, updates) {
	const out = { ...existing };
	let changed = false;
	const log = [];

	if (updates.qualifying) {
		const prev = existing.qualifying_record;
		if (!prev || updates.qualifying.time_s < prev.time_s) {
			out.qualifying_record = updates.qualifying;
			changed = true;
			log.push(
				prev
					? `qualifying: ${prev.time_s}s → ${updates.qualifying.time_s}s (${updates.qualifying.driver})`
					: `qualifying: seeded ${updates.qualifying.time_s}s (${updates.qualifying.driver})`,
			);
		}
	}
	if (updates.raceLap) {
		const prev = existing.race_lap_record;
		if (!prev || updates.raceLap.time_s < prev.time_s) {
			out.race_lap_record = updates.raceLap;
			changed = true;
			log.push(
				prev
					? `race lap: ${prev.time_s}s → ${updates.raceLap.time_s}s (${updates.raceLap.driver})`
					: `race lap: seeded ${updates.raceLap.time_s}s (${updates.raceLap.driver})`,
			);
		}
	}
	if (updates.winner) {
		const prev = existing.previous_winner;
		if (
			!prev ||
			prev.year !== updates.winner.year ||
			prev.driver !== updates.winner.driver
		) {
			out.previous_winner = updates.winner;
			changed = true;
			log.push(
				`winner: ${prev ? `${prev.driver} (${prev.year})` : '—'} → ${updates.winner.driver} (${updates.winner.year})`,
			);
		}
	}

	return { out, changed, log };
}

async function main() {
	const args = parseArgs(process.argv.slice(2));
	console.log(`Updating track records from ${args.year}${args.circuit ? ` (circuit=${args.circuit})` : ''}${args.dryRun ? ' [dry-run]' : ''}`);

	const meetings = await fetchJson(`https://api.openf1.org/v1/meetings?year=${args.year}`);

	let touched = 0;
	for (const meeting of meetings) {
		const key = NAME_MAP[meeting.circuit_short_name];
		if (!key) {
			console.log(`  skip: unknown circuit "${meeting.circuit_short_name}"`);
			continue;
		}
		if (args.circuit && args.circuit !== key) continue;

		let existing;
		try {
			existing = await loadTrack(key);
		} catch (e) {
			console.warn(`  skip ${key}: no track file (${e.message})`);
			continue;
		}

		let updates;
		try {
			updates = await processMeeting(meeting, args.year, existing.data);
		} catch (e) {
			console.warn(`  skip ${key}: ${e.message}`);
			continue;
		}

		const { out, changed, log } = mergeRecords(existing.data, updates);
		if (!changed) {
			console.log(`  ${key}: no changes`);
			continue;
		}
		console.log(`  ${key}:`);
		for (const line of log) console.log(`    - ${line}`);
		if (!args.dryRun) await writeTrack(existing.file, out);
		touched += 1;
	}

	console.log(`\nDone. ${touched} circuit file(s) ${args.dryRun ? 'would change' : 'updated'}.`);
}

main().catch((e) => {
	console.error(e);
	process.exit(1);
});
