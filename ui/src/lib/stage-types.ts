// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import type { Quad } from './quad.ts';
import type { Band, Pt } from './types.ts';

/** One crop as the source view draws it. */
export interface StageCrop {
  id: number;
  /** Output rank, 0 while excluded. */
  order: number;
  quad: Quad;
  include: boolean;
  band: Band | null;
}

/** One entry of a popup menu. */
export interface MenuEntry {
  key: string;
  label: string;
  /** Why the entry is off, shown as its description. */
  disabled?: string | false;
  hint?: string;
  danger?: boolean;
  run: () => void;
}

/** The live preview of a cut: the two pieces (null while the cut is not valid) and the line. */
export interface CutPreview {
  pieces: [Quad, Quad] | null;
  line: [Pt, Pt];
}
