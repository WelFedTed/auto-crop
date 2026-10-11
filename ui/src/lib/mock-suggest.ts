// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The browser mock's stand-in for `Engine::suggest_curves`: pure, so it is tested without a canvas.
//
// It is NOT a fitter. For the curved sample it answers with the sample's own analytic edges (what a good fit would be
// close to) when the page's corners are near the real ones, with a deliberately weaker bottom edge (the sample's
// detector flags it too); for any other page, or corners that are far off, it answers like the engine does when it
// refuses the fit: `fellBack` with the crop's straight edges. The honesty fields are filled the way the engine fills
// them (curvature as the largest distance from the chord in percent of the chord, `maxCurvature` only from edges that
// pass the gates), so the UI is exercised on the same shapes of answer.

import { curvedEdges } from './curved-model.ts';
import { cornersOf, curvesFromQuad, EDGE_KEYS, polyline } from './curve.ts';
import type { CurveSet, CurveSuggestion, EdgeNumbers, Pt } from './types.ts';

/** How far (normalised, per corner) the crop's corners may be from the sample's real ones and still be fitted. */
export const MOCK_CAPTURE = 0.08;

const dist = (a: Pt, b: Pt): number => Math.hypot(a.x - b.x, a.y - b.y);

/** The largest distance of the spline through `pts` from its chord, in percent of the chord, in pixels `scale`. */
export function curvaturePct(pts: readonly Pt[], scale: readonly [number, number]): number {
  if (pts.length <= 2) return 0;
  const px = (p: Pt): Pt => ({ x: p.x * scale[0], y: p.y * scale[1] });
  const a = px(pts[0]);
  const b = px(pts[pts.length - 1]);
  const chord = dist(a, b);
  if (chord < 1e-9) return 0;
  let worst = 0;
  for (const q of polyline(pts, 16)) {
    const p = px(q);
    // distance of p from the infinite line a-b
    const d = Math.abs((b.x - a.x) * (a.y - p.y) - (a.x - p.x) * (b.y - a.y)) / chord;
    worst = Math.max(worst, d);
  }
  return (worst / chord) * 100;
}

/** The curvature figures the engine reports for a curve set, per edge (top, right, bottom, left). */
export function curvatures(c: CurveSet, scale: readonly [number, number]): EdgeNumbers {
  return EDGE_KEYS.map((k) => curvaturePct(c[k], scale)) as EdgeNumbers;
}

export interface MockSuggestInput {
  /** The page is the curved sample (the only page the mock can "see" a curve on). */
  curvedSample: boolean;
  /** The crop's corners TL, TR, BR, BL, normalised (a curved crop's end points). */
  corners: [Pt, Pt, Pt, Pt];
  quarterTurns: number;
  mirror: boolean;
  /** The picture's size in pixels (for the percentages). */
  size: readonly [number, number];
}

export function mockSuggestion(input: MockSuggestInput): CurveSuggestion {
  const { corners, quarterTurns, mirror, size } = input;
  const straight = (): CurveSuggestion => ({
    quad: corners.map((p) => ({ ...p })) as CurveSuggestion['quad'],
    curves: curvesFromQuad(corners, quarterTurns, mirror),
    confidence: [0, 0, 0, 0],
    curvature: [0, 0, 0, 0],
    bulge: [0, 0, 0, 0],
    supportGain: [0, 0, 0, 0],
    maxCurvature: 0,
    looksCurved: false,
    fellBack: true,
  });
  if (!input.curvedSample) return straight();

  const [top, right, bottom, left] = curvedEdges(5);
  const truth = [top[0], right[0], bottom[0], left[0]];
  if (corners.some((p, i) => dist(p, truth[i]) > MOCK_CAPTURE)) return straight();

  const curves: CurveSet = { top, right, bottom, left, quarterTurns, mirror };
  const quad = cornersOf(curves) as CurveSuggestion['quad'];
  const confidence: EdgeNumbers = [0.92, 0.9, 0.62, 0.88];
  const curvature = curvatures(curves, size);
  // percent of the page's shorter side: the chords in pixels, the shorter of (top, bottom) and (left, right) on average
  const chord = EDGE_KEYS.map((k) => {
    const e = curves[k];
    return dist({ x: e[0].x * size[0], y: e[0].y * size[1] }, { x: e[e.length - 1].x * size[0], y: e[e.length - 1].y * size[1] });
  });
  const shorter = Math.min((chord[0] + chord[2]) / 2, (chord[1] + chord[3]) / 2);
  const bulge = curvature.map((c, k) => (((c / 100) * chord[k]) / Math.max(1, shorter)) * 100) as EdgeNumbers;
  const supportGain = curvature.map((c) => (c >= 1 ? 0.8 : 0.02)) as EdgeNumbers;
  const maxCurvature = Math.max(0, ...curvature.map((c, k) => (confidence[k] >= 0.7 && c >= 1 && supportGain[k] >= 0.35 ? c : 0)));
  return { quad, curves, confidence, curvature, bulge, supportGain, maxCurvature, looksCurved: maxCurvature > 0, fellBack: false };
}
