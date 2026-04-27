#!/usr/bin/env node
/**
 * Populate data/tracks/{key}.json from OpenF1 meetings + MultiViewer circuit_info_url.
 *
 * For each unique circuit in the target year:
 *   - GET OpenF1 /v1/meetings to find circuit_info_url
 *   - GET the MultiViewer JSON
 *   - Extract reference lap polyline (x[], y[]), corners (with angle), and rotation
 *   - Write data/tracks/{key}.json
 *
 * Existing `name` fields on turns are preserved across reruns (keyed by
 * `${number}${letter ?? ''}`), so hand-curated corner names don't get wiped.
 *
 * Usage: node scripts/fetch-tracks.mjs [--year 2026]
 */

import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..');
const TRACKS_DIR = path.join(REPO_ROOT, 'data', 'tracks');

const args = process.argv.slice(2);
const yearIdx = args.indexOf('--year');
const YEAR = yearIdx >= 0 ? Number(args[yearIdx + 1]) : new Date().getFullYear();

// OpenF1 circuit_short_name -> our canonical track key (filename stem).
// Must stay in sync with resolve_circuit() in crates/f1core/src/domain/track.rs.
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

async function loadExisting() {
	const byKey = {};
	let files = [];
	try {
		files = await fs.readdir(TRACKS_DIR);
	} catch {
		return byKey;
	}
	for (const f of files) {
		if (!f.endsWith('.json')) continue;
		const key = f.replace(/\.json$/, '');
		try {
			const raw = await fs.readFile(path.join(TRACKS_DIR, f), 'utf-8');
			const parsed = JSON.parse(raw);
			const names = {};
			for (const t of parsed.turns ?? []) {
				if (!t.name) continue;
				const tag = `${t.number}${t.letter ?? ''}`;
				names[tag] = t.name;
			}
			byKey[key] = {
				names,
				race_laps: parsed.race_laps,
				length_km: parsed.length_km,
				qualifying_record: parsed.qualifying_record,
				race_lap_record: parsed.race_lap_record,
				previous_winner: parsed.previous_winner,
			};
		} catch {
			// ignore malformed existing files — they'll be overwritten
		}
	}
	return byKey;
}

async function fetchJson(url) {
	const res = await fetch(url);
	if (!res.ok) throw new Error(`${url} -> ${res.status}`);
	return res.json();
}

async function main() {
	console.log(`Fetching circuits for ${YEAR}...`);
	const meetings = await fetchJson(`https://api.openf1.org/v1/meetings?year=${YEAR}`);

	const targets = new Map();
	for (const m of meetings) {
		const key = NAME_MAP[m.circuit_short_name];
		if (!key) {
			console.warn(`  skip: unknown circuit_short_name "${m.circuit_short_name}"`);
			continue;
		}
		if (targets.has(key)) continue;
		targets.set(key, { url: m.circuit_info_url, short: m.circuit_short_name });
	}

	const existing = await loadExisting();
	await fs.mkdir(TRACKS_DIR, { recursive: true });

	let written = 0;
	for (const [key, { url, short }] of targets) {
		let info;
		try {
			info = await fetchJson(url);
		} catch (e) {
			console.warn(`  skip ${key} (${short}): ${e.message}`);
			continue;
		}

		const xs = info.x ?? [];
		const ys = info.y ?? [];
		if (xs.length === 0 || xs.length !== ys.length) {
			console.warn(`  skip ${key}: empty or mismatched x/y arrays`);
			continue;
		}
		const points = xs.map((x, i) => [x, ys[i]]);

		const existingForKey = existing[key] ?? {};
		const existingNames = existingForKey.names ?? {};
		const turns = (info.corners ?? []).map((c) => {
			const tag = `${c.number}${c.letter ?? ''}`;
			const out = {
				number: c.number,
				x: c.trackPosition.x,
				y: c.trackPosition.y,
				angle: c.angle,
			};
			if (c.letter) out.letter = c.letter;
			if (existingNames[tag]) out.name = existingNames[tag];
			return out;
		});

		const body = {
			rotation: info.rotation ?? 0,
			...(existingForKey.race_laps != null && { race_laps: existingForKey.race_laps }),
			...(existingForKey.length_km != null && { length_km: existingForKey.length_km }),
			...(existingForKey.qualifying_record != null && {
				qualifying_record: existingForKey.qualifying_record,
			}),
			...(existingForKey.race_lap_record != null && {
				race_lap_record: existingForKey.race_lap_record,
			}),
			...(existingForKey.previous_winner != null && {
				previous_winner: existingForKey.previous_winner,
			}),
			points,
			turns,
		};
		const file = path.join(TRACKS_DIR, `${key}.json`);
		await fs.writeFile(file, JSON.stringify(body, null, 2) + '\n');
		console.log(
			`  wrote ${key} — ${points.length} pts, ${turns.length} turns, rot=${body.rotation}`,
		);
		written += 1;
	}

	console.log(`\nDone. ${written} circuit file(s) written to ${TRACKS_DIR}`);
}

main().catch((e) => {
	console.error(e);
	process.exit(1);
});
