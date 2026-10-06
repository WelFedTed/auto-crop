<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { api, cropImageUrl, imageUrl } from '../lib/backend.ts';
  import ChipBar from '../lib/components/ChipBar.svelte';
  import CropStage from '../lib/components/CropStage.svelte';
  import FirstSplitSheet from '../lib/components/FirstSplitSheet.svelte';
  import FirstWriteSheet from '../lib/components/FirstWriteSheet.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import ItemDock from '../lib/components/ItemDock.svelte';
  import Menu from '../lib/components/Menu.svelte';
  import ResultInset from '../lib/components/ResultInset.svelte';
  import Ruler from '../lib/components/Ruler.svelte';
  import ScanBanner from '../lib/components/ScanBanner.svelte';
  import TierBadge from '../lib/components/TierBadge.svelte';
  import { cutFromDrag, cutPieces, cutProblem as cutProblemOf, cutLine, halvesCut, mergedQuad } from '../lib/geometry.ts';
  import type { StageTool } from '../lib/gesture.ts';
  import {
    GestureClock,
    bannerState,
    collisionNotice,
    hasItemLayer,
    includedCrops,
    isSplitScan,
    moveTarget,
    nameSummary,
    nextIncluded,
    pickSelection,
    plannedNames,
    saveGate,
    slotToIndex,
  } from '../lib/items.ts';
  import { cloneEdit, normaliseAngle, parsePercent, setCornerPercent, toPercent, turnQuarter, type Quad } from '../lib/quad.ts';
  import { canAccept, defaultQueue, needsDrawCrop, nextToReview, reasonLine } from '../lib/review.ts';
  import { href, navigate } from '../lib/router.svelte.ts';
  import type { CutPreview, MenuEntry, StageCrop } from '../lib/stage-types.ts';
  import { codeOf, store, type ScanSave } from '../lib/store.svelte.ts';
  import { FALLBACK_HOLD, S, errorMessage, holdAction, holdCause, holdTitle, noticeText } from '../lib/strings.ts';
  import type { Band, CropView, Cut, Edit, ItemView, Pt, SplitPolicy, SplitProfile } from '../lib/types.ts';

  let { id }: { id: number } = $props();

  const item = $derived<ItemView | undefined>(store.byId.get(id));
  const cls = $derived(store.classified.find((x) => x.item.id === id));
  const tier = $derived(cls?.tier ?? 'analysing');
  const decision = $derived(cls?.decision ?? null);

  // ---- queue -------------------------------------------------------------------------------
  // The editor is re-created for every item (App keys it by id), so reading `id` once here is intended.
  const startId = untrack(() => id);
  /** The undo position when this editor opened, for "Undo my changes to this item". */
  const entryPosition = untrack(() => store.byId.get(id)?.historyPosition ?? 0);
  if (!store.queue.includes(startId)) store.queue = defaultQueue(store.classified);
  const queue = store.queue;
  const position = queue.indexOf(startId) + 1;

  function adjacent(dir: 1 | -1): number | null {
    let at = queue.indexOf(id);
    for (;;) {
      at += dir;
      if (at < 0 || at >= queue.length) return null;
      const it = store.byId.get(queue[at]);
      if (it && it.status === 'ready') return queue[at];
    }
  }

  const prevId = $derived(adjacent(-1));
  const nextId = $derived(adjacent(1));

  function goTo(target: number): void {
    store.lastOpened = target;
    navigate(`/item/${target}`);
  }

  function leave(): void {
    store.lastOpened = id;
    navigate('/grid');
  }

  function goNextToReview(): void {
    const needs = new Set(store.classified.filter((x) => x.needs).map((x) => x.item.id));
    const target = nextToReview(queue, id, needs);
    if (target === null) {
      store.toast(S.editor.noMoreFlagged);
      leave();
    } else goTo(target);
  }

  // ---- selection ----------------------------------------------------------------------------
  let selectedId = $state<number | null>(null);
  $effect(() => {
    const next = item ? pickSelection(item, selectedId) : null;
    if (next !== selectedId) selectedId = next;
  });
  const crops = $derived(item?.crops ?? []);
  const included = $derived(item ? includedCrops(item) : []);
  const sel = $derived<CropView | null>(crops.find((c) => c.id === selectedId && c.include) ?? null);
  const multi = $derived(crops.length > 1);

  // ---- edits: optimistic draft plus one serial chain of calls --------------------------------
  let draft = $state.raw<{ crop: number; edit: Edit } | null>(null);
  let previewQuad = $state.raw<Quad | null>(null);
  let liveAngle = $state<number | null>(null);
  let inflight = $state(0);
  let chain: Promise<unknown> = Promise.resolve();
  let sayTimer: ReturnType<typeof setTimeout> | undefined;
  const gestures = new GestureClock(500);

  const edit = $derived<Edit | null>(draft && draft.crop === selectedId ? draft.edit : (sel?.edit ?? null));
  const shownEdit = $derived<Edit | null>(
    edit ? { quad: previewQuad ?? edit.quad, quarterTurns: edit.quarterTurns, fineDeg: liveAngle ?? edit.fineDeg } : null,
  );
  const busy = $derived(previewQuad !== null || liveAngle !== null || inflight > 0);

  function say(text: string): void {
    clearTimeout(sayTimer);
    sayTimer = setTimeout(() => store.announce(text), 150);
  }

  function enqueue(work: () => Promise<ItemView | void>, optimistic: { crop: number; edit: Edit } | null): void {
    if (optimistic) draft = { crop: optimistic.crop, edit: cloneEdit(optimistic.edit) };
    inflight++;
    chain = chain.then(async () => {
      try {
        const v = await work();
        if (v) store.upsert(v);
      } catch (e) {
        console.error(e);
        const code = codeOf(e);
        store.toast(code ? errorMessage(code) : S.editor.editFailed, 'error');
      } finally {
        inflight--;
        if (inflight === 0) draft = null;
      }
    });
  }

  /** One item operation: the engine records ONE undo step; `after` sees the new view and the one before it. */
  function runOp(work: () => Promise<ItemView>, after?: (v: ItemView, before: ItemView) => void): void {
    const before = item;
    enqueue(async () => {
      const v = await work();
      if (before && after) after(v, before);
      return v;
    }, null);
  }

  function commitEdit(next: Edit, label: string, announce?: string, source: 'drag' | 'key' = 'drag'): void {
    if (!sel) return;
    const gesture = source === 'key' ? gestures.next(`${id}:${sel.id}`, performance.now()) : null;
    const cropId = sel.id;
    enqueue(() => api.setCropEdit(id, cropId, next, 'end', label, gesture), { crop: cropId, edit: next });
    if (announce) say(announce);
  }

  function commitQuad(quad: Quad, label: string, announce: string, source: 'drag' | 'key'): void {
    if (!edit) return;
    commitEdit({ ...cloneEdit(edit), quad }, label, announce, source);
  }

  function commitAngle(deg: number, label = S.editor.labels.rotate, source: 'drag' | 'key' = 'drag'): void {
    if (!edit || !sel) return;
    const v = normaliseAngle(deg);
    const cropId = sel.id;
    const gesture = source === 'key' ? gestures.next(`${id}:${cropId}:angle`, performance.now()) : null;
    enqueue(() => api.setCropAngle(id, cropId, v, gesture), { crop: cropId, edit: { ...cloneEdit(edit), fineDeg: v } });
    say(S.editor.angleSpoken(v));
    void label;
  }

  function turn(dir: 1 | -1): void {
    if (!edit || !sel) return;
    const cropId = sel.id;
    enqueue(() => api.turnCrop(id, cropId, dir > 0), { crop: cropId, edit: { ...cloneEdit(edit), quarterTurns: turnQuarter(edit.quarterTurns, dir) } });
    say(dir > 0 ? S.editor.rotateRight90 : S.editor.rotateLeft90);
  }

  function flip(): void {
    if (!sel) return;
    const cropId = sel.id;
    runOp(() => api.flipCrop(id, cropId));
  }

  function setCorner(i: number, axis: 'x' | 'y', text: string): void {
    if (!edit) return;
    const v = parsePercent(text);
    if (v === null) return;
    commitEdit({ ...cloneEdit(edit), quad: setCornerPercent(edit.quad, i, axis, v) }, S.editor.labels.editCorner, `${S.editor.corners[i]} ${axis} ${v.toFixed(1)}%`);
  }

  function doUndo(): void {
    if (!item?.canUndo) return;
    const label = item.undoLabel ?? '';
    const wasSaved = !!item.saved;
    enqueue(async () => {
      const v = await api.undo(id);
      store.announce(S.editor.undone(label));
      if (wasSaved) store.toast(S.editor.undoSavedToast, 'info', { label: S.editor.restoreOriginalFile, run: () => navigate('/backups') });
      return v;
    }, null);
  }

  function doRedo(): void {
    if (!item?.canRedo) return;
    const label = item.redoLabel ?? '';
    enqueue(async () => {
      const v = await api.redo(id);
      store.announce(S.editor.redone(label));
      return v;
    }, null);
  }

  /** Whole-image reset: for one crop it is "Reset to auto"; for several it resets every item. */
  function doReset(): void {
    enqueue(() => api.resetToAuto(id), null);
    store.announce(S.editor.resetToAuto);
  }

  function doDrawCrop(): void {
    enqueue(() => api.drawCrop(id), null);
  }

  // ---- items: select, remove, restore, reorder -----------------------------------------------------
  const bandWord = (b: Band | null): string => S.tier[b ?? 'check'];
  const reasonOf = (c: CropView): string | null => {
    if (c.band === 'good') return null;
    const r = c.confidence?.reasons[0];
    return r ? holdTitle(r) : FALLBACK_HOLD[c.band === 'failed' ? 'failed' : 'check'].title;
  };

  let stage = $state<{ zoomBy: (f: number) => void; fit: () => void; actualSize: () => void; zoomToQuad: (q: readonly Pt[]) => void } | null>(null);
  let tool = $state<StageTool>('none');
  let mergeIds = $state.raw<number[]>([]);
  let cut = $state.raw<Cut | null>(null);

  function selectCrop(cropId: number, fromStage: boolean): void {
    if (tool === 'merge') {
      toggleMerge(cropId);
      return;
    }
    const c = crops.find((x) => x.id === cropId);
    if (!c || !c.include) return;
    selectedId = cropId;
    store.announce(S.items.selected(c.order, bandWord(c.band)));
    // The chip bar zooms to the item; a tap on the picture already sees it.
    if (!fromStage && c.edit) stage?.zoomToQuad(c.edit.quad);
  }

  function selectAfterChange(v: ItemView, before: ItemView, preferNew: boolean): void {
    const old = new Set(before.crops.map((c) => c.id));
    const fresh = v.crops.filter((c) => !old.has(c.id) && c.include);
    if (preferNew && fresh.length > 0) {
      selectedId = fresh[0].id;
      const q = fresh[0].edit?.quad;
      if (q) stage?.zoomToQuad(q);
    } else selectedId = pickSelection(v, selectedId);
  }

  function removeCrop(cropId: number): void {
    const c = crops.find((x) => x.id === cropId);
    if (!c) return;
    const at = included.findIndex((x) => x.id === cropId);
    const neighbour = included[at + 1] ?? included[at - 1] ?? null;
    runOp(
      () => api.removeCrop(id, cropId),
      (v) => {
        if (selectedId === cropId) selectedId = neighbour && neighbour.id !== cropId ? neighbour.id : pickSelection(v, null);
        say(S.items.removed(c.order));
      },
    );
  }

  function restoreCrop(cropId: number): void {
    runOp(
      () => api.restoreCrop(id, cropId),
      (v) => {
        selectedId = cropId;
        const n = v.crops.find((c) => c.id === cropId)?.order ?? 0;
        say(S.items.restored(n));
        const q = v.crops.find((c) => c.id === cropId)?.edit?.quad;
        if (q) stage?.zoomToQuad(q);
      },
    );
  }

  function resetCrop(cropId: number): void {
    runOp(() => api.revertCrop(id, cropId, { kind: 'auto' }));
    say(S.items.resetToAuto);
  }

  function revertCropSession(cropId: number): void {
    runOp(() => api.revertCrop(id, cropId, { kind: 'step', position: entryPosition }));
  }

  function moveCrop(cropId: number, dir: -1 | 1): void {
    if (!item) return;
    const to = moveTarget(item, cropId, dir);
    if (to === null) return;
    runOp(
      () => api.moveCrop(id, cropId, to),
      (v) => {
        const c = v.crops.find((x) => x.id === cropId);
        say(S.items.moved(c?.order ?? 0, v.split?.included ?? 0));
      },
    );
  }

  function reorderCrop(cropId: number, slot: number): void {
    if (!item) return;
    const to = slotToIndex(item, cropId, slot);
    runOp(
      () => api.moveCrop(id, cropId, to),
      (v) => {
        const c = v.crops.find((x) => x.id === cropId);
        say(S.items.moved(c?.order ?? 0, v.split?.included ?? 0));
      },
    );
  }

  function useReading(): void {
    runOp(() => api.useReadingOrder(id));
  }

  // ---- tools: add, merge, cut -----------------------------------------------------------------------
  function cancelTool(): void {
    tool = 'none';
    mergeIds = [];
    cut = null;
  }

  function startAdd(kind: 'tap' | 'draw' | 'inset'): void {
    cancelTool();
    if (kind === 'inset') {
      runOp(
        () => api.addCrop(id, null, null),
        (v, b) => {
          selectAfterChange(v, b, true);
          say(S.items.added(v.crops.find((c) => c.id === selectedId)?.order ?? 0));
        },
      );
      return;
    }
    tool = kind === 'tap' ? 'add-tap' : 'add-draw';
  }

  function addAt(p: Pt): void {
    tool = 'none';
    runOp(
      () => api.addCrop(id, null, p),
      (v, b) => {
        selectAfterChange(v, b, true);
        say(S.items.added(v.crops.find((c) => c.id === selectedId)?.order ?? 0));
      },
    );
  }

  function addBox(q: Quad): void {
    tool = 'none';
    runOp(
      () => api.addCrop(id, q, null),
      (v, b) => {
        selectAfterChange(v, b, true);
        say(S.items.added(v.crops.find((c) => c.id === selectedId)?.order ?? 0));
      },
    );
  }

  function startMerge(ids?: number[]): void {
    const first = ids ?? (sel ? [sel.id] : []);
    cancelTool();
    tool = 'merge';
    mergeIds = first;
  }

  function toggleMerge(cropId: number): void {
    const c = crops.find((x) => x.id === cropId);
    if (!c?.include) return;
    mergeIds = mergeIds.includes(cropId) ? mergeIds.filter((x) => x !== cropId) : [...mergeIds, cropId];
    store.announce(S.items.mergeHint(mergeIds.length));
  }

  const mergePreview = $derived<Quad | null>(
    tool === 'merge' && mergeIds.length >= 2 && item
      ? mergedQuad(
          mergeIds.map((m) => crops.find((c) => c.id === m)?.edit?.quad ?? []).filter((q) => q.length === 4),
          Math.max(1, item.width),
          Math.max(1, item.height),
        )
      : null,
  );

  function confirmMerge(): void {
    const ids = [...mergeIds];
    cancelTool();
    runOp(
      () => api.mergeCrops(id, ids),
      (v, b) => {
        selectAfterChange(v, b, true);
        say(S.items.merged(v.crops.find((c) => c.id === selectedId)?.order ?? 0));
      },
    );
  }

  function startCut(): void {
    if (!sel?.edit) return;
    const q = sel.edit.quad;
    const wide = Math.hypot(q[1].x - q[0].x, q[1].y - q[0].y) * (item?.width ?? 1) >= Math.hypot(q[3].x - q[0].x, q[3].y - q[0].y) * (item?.height ?? 1);
    cancelTool();
    tool = 'cut';
    cut = halvesCut(wide ? 'vertical' : 'horizontal');
    // The cut panel changes the stage's height; frame the item once it has settled.
    void tick().then(() => stage?.zoomToQuad(q));
  }

  function cutDrag(a: Pt, b: Pt): void {
    if (!sel?.edit) return;
    const c = cutFromDrag(sel.edit.quad as Quad, a, b);
    if (c) cut = c;
  }

  const cutIssue = $derived(tool === 'cut' && cut && sel?.edit && item ? cutProblemOf(sel.edit.quad as Quad, cut, Math.max(1, item.width), Math.max(1, item.height)) : null);
  const cutPreview = $derived<CutPreview | null>(
    tool === 'cut' && cut && sel?.edit ? { pieces: cutPieces(sel.edit.quad as Quad, cut), line: cutLine(sel.edit.quad as Quad, cut) } : null,
  );

  function confirmCut(): void {
    if (!cut || !sel) return;
    const c = cut;
    const cropId = sel.id;
    cancelTool();
    runOp(
      () => api.cutCrop(id, cropId, c),
      (v, b) => {
        selectAfterChange(v, b, true);
        say(S.items.cutDone);
      },
    );
  }

  // ---- item menu -------------------------------------------------------------------------------------
  let menu = $state<{ id: number; x: number; y: number } | null>(null);

  const menuEntries = $derived.by<MenuEntry[]>(() => {
    if (!menu || !item) return [];
    const c = crops.find((x) => x.id === menu!.id);
    if (!c) return [];
    if (!c.include) return [{ key: 'restore', label: S.items.restore, hint: S.items.addAsItem, run: () => restoreCrop(c.id) }];
    const next = nextIncluded(item, c.id);
    return [
      { key: 'remove', label: S.items.remove, hint: S.items.removeHelp, danger: true, run: () => removeCrop(c.id) },
      { key: 'reset', label: S.items.resetToAuto, hint: S.items.resetHelp, disabled: c.autoEdit ? false : S.items.reviewed, run: () => resetCrop(c.id) },
      { key: 'revert', label: S.items.revertSession, run: () => revertCropSession(c.id) },
      { key: 'earlier', label: S.items.moveEarlier, disabled: moveTarget(item, c.id, -1) === null ? S.items.moveEarlier + ': first' : false, run: () => moveCrop(c.id, -1) },
      { key: 'later', label: S.items.moveLater, disabled: moveTarget(item, c.id, 1) === null ? S.items.moveLater + ': last' : false, run: () => moveCrop(c.id, 1) },
      { key: 'merge', label: S.items.mergeWithNext, disabled: next ? false : S.items.mergeWithNext + ': no next item', run: () => startMerge(next ? [c.id, next.id] : [c.id]) },
      { key: 'cut', label: S.items.cut, run: () => { selectedId = c.id; void tick().then(startCut); } },
      { key: 'left', label: S.items.turnLeft, run: () => { selectedId = c.id; void tick().then(() => turn(-1)); } },
      { key: 'right', label: S.items.turnRight, run: () => { selectedId = c.id; void tick().then(() => turn(1)); } },
      { key: 'flip', label: S.items.flip, run: () => { selectedId = c.id; void tick().then(flip); } },
    ];
  });

  function openMenu(cropId: number, at: { x: number; y: number }): void {
    menu = { id: cropId, x: at.x, y: at.y };
  }

  function closeMenu(restoreFocus: boolean): void {
    const m = menu;
    menu = null;
    if (restoreFocus && m) {
      void tick().then(() => document.querySelector<HTMLElement>(`[data-main="${m.id}"], [data-restore-chip="${m.id}"]`)?.focus());
    }
  }

  // ---- split settings -----------------------------------------------------------------------------------
  function setPolicy(policy: SplitPolicy): void {
    if (!item?.split || item.split.policy === policy) return;
    const label = policy === 'never' ? S.items.policyNever : policy === 'always' ? S.items.policyAlways : S.items.policyAuto;
    cancelTool();
    runOp(
      () => api.redetect(id, { policy }),
      (v) => {
        selectedId = pickSelection(v, null);
        say(S.items.policyDone(label));
      },
    );
  }

  function setProfile(profile: SplitProfile): void {
    if (!item?.split || item.split.profile === profile) return;
    runOp(() => api.redetect(id, { profile }));
  }

  // ---- decisions ------------------------------------------------------------------------------------------
  const split = $derived(item ? isSplitScan(item) : false);
  const banner = $derived(item ? bannerState(item, store.settings) : ({ kind: 'none' } as const));

  async function acceptSplit(): Promise<boolean> {
    const ok = await store.acceptScan(id);
    if (ok) store.decide([id], 'accepted');
    return ok;
  }

  function accept(): boolean {
    if (!item) return false;
    if (!canAccept(item, store.strictness)) {
      store.toast(S.editor.cannotAcceptFailed);
      return false;
    }
    if (split) {
      void acceptSplit();
      return true;
    }
    store.decide([id], 'accepted');
    return true;
  }

  function toggleAccepted(): void {
    if (split) {
      if (item?.split?.accepted) void store.unacceptScan(id);
      else void acceptSplit();
      return;
    }
    if (decision === 'accepted') store.decide([id], null);
    else if (accept()) store.announce(S.editor.acceptedToast);
  }

  async function acceptAndNext(): Promise<void> {
    if (!item) return;
    if (split) {
      if (item.split?.accepted || (await acceptSplit())) goNextToReview();
      return;
    }
    if (accept()) goNextToReview();
  }

  function skip(): void {
    if (decision === 'skipped') {
      store.decide([id], null);
      return;
    }
    store.decide([id], 'skipped');
    store.announce(S.editor.skipped);
    goNextToReview();
  }

  function reviewItems(): void {
    const target = included.find((c) => (c.band ?? 'check') !== 'good') ?? included[0];
    if (!target) return;
    selectedId = target.id;
    if (target.edit) stage?.zoomToQuad(target.edit.quad);
    void tick().then(() => document.querySelector<HTMLElement>(`[data-main="${target.id}"]`)?.focus());
  }

  // ---- saving this scan -----------------------------------------------------------------------------------------
  let lastSave = $state.raw<ScanSave | null>(null);
  let sheetOpen = $state(false);
  let splitSheetOpen = $state(false);
  let pendingTarget = $state<'copy' | 'replace'>('replace');
  const gate = $derived(item ? saveGate(item, store.settings) : ({ replace: 'no-crop' } as const));
  const planned = $derived(item ? plannedNames(item) : []);

  function requestSave(target: 'copy' | 'replace'): void {
    pendingTarget = target;
    lastSave = null;
    // A copy removes and overwrites nothing, so it never needs the "before the first save" sheet.
    if (target === 'replace' && !store.settings.firstWriteAck) {
      sheetOpen = true;
      return;
    }
    if (target === 'replace' && split && !store.firstSplitAck) {
      splitSheetOpen = true;
      return;
    }
    void doSave(target);
  }

  async function chooseMode(copy: boolean): Promise<void> {
    // The sheet speaks of the global mode; here the person pressed a specific button, so keep their choice for
    // this save and store only the acknowledgement plus the mode they picked on the sheet.
    const ok = await store.updateSettings({ saveAsCopy: copy, firstWriteAck: true });
    sheetOpen = false;
    if (!ok) return;
    const target = copy ? 'copy' : 'replace';
    if (target === 'replace' && split && !store.firstSplitAck) {
      splitSheetOpen = true;
      return;
    }
    void doSave(target);
  }

  async function doSave(target: 'copy' | 'replace'): Promise<void> {
    const r = await store.saveScan(id, target);
    lastSave = r;
    if (!r) return;
    if (r.outcome.ok) store.announce(S.save.savedAs(nameSummary(r.outcome.saved?.outputs.length ? r.outcome.saved.outputs : [r.outcome.saved?.output ?? ''])));
    else store.announce(errorMessage(r.outcome.error));
  }

  // ---- compare (hold or toggle) ------------------------------------------------------------------------------------
  let compareLatched = $state(false);
  let compareHeld = $state(false);
  let compareDownAt = 0;
  const compare = $derived(compareLatched || compareHeld);

  function compareDown(): void {
    compareHeld = true;
    compareDownAt = performance.now();
  }

  function compareUp(): void {
    if (!compareHeld) return;
    compareHeld = false;
    if (performance.now() - compareDownAt < 350) compareLatched = !compareLatched;
  }

  // ---- keyboard -------------------------------------------------------------------------------------------------------
  function typing(t: EventTarget | null): boolean {
    const el = t as HTMLElement | null;
    if (!el) return false;
    return el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA' || el.isContentEditable;
  }

  function stepSelection(dir: -1 | 1): void {
    if (included.length === 0) return;
    const at = included.findIndex((c) => c.id === selectedId);
    const next = included[(at + dir + included.length) % included.length];
    selectCrop(next.id, false);
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.defaultPrevented) return;
    const t = e.target instanceof HTMLElement ? e.target : null;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === 'z' && !typing(t)) {
      e.preventDefault();
      if (e.shiftKey) doRedo();
      else doUndo();
      return;
    }
    if (mod && e.key.toLowerCase() === 'y' && !typing(t)) {
      e.preventDefault();
      doRedo();
      return;
    }
    if (mod && e.key === 'Enter') {
      e.preventDefault();
      void acceptAndNext();
      return;
    }
    if (typing(t) || mod || e.altKey) return;
    if (e.key === 'Escape') {
      if (tool !== 'none') {
        cancelTool();
        return;
      }
      leave();
      return;
    }
    const onControl = !!t?.closest('[data-handle], [role="slider"], [data-chipbar], [data-menu]');
    if (!onControl && e.key === 'ArrowLeft' && prevId !== null) {
      e.preventDefault();
      goTo(prevId);
    } else if (!onControl && e.key === 'ArrowRight' && nextId !== null) {
      e.preventDefault();
      goTo(nextId);
    } else if (e.key === 'x' || e.key === 'X') skip();
    else if (e.key === 'r') turn(1);
    else if (e.key === 'R') turn(-1);
    else if (e.key === 'i' || e.key === 'I') startAdd('inset');
    else if (e.key === 'm' || e.key === 'M') {
      if (included.length >= 2) startMerge();
    } else if (e.key === 'k' || e.key === 'K') startCut();
    else if (e.key === ',') stepSelection(-1);
    else if (e.key === '.') stepSelection(1);
    else if (e.key === 'b' || e.key === 'B') {
      if (!e.repeat) compareHeld = true;
    } else if (e.key === '+' || e.key === '=') stage?.zoomBy(1.25);
    else if (e.key === '-') stage?.zoomBy(1 / 1.25);
    else if (e.key === '0' || e.key === 'z' || e.key === 'Z') stage?.fit();
    else if (e.key === '1') stage?.actualSize();
  }

  function onKeyup(e: KeyboardEvent): void {
    if ((e.key === 'b' || e.key === 'B') && compareHeld) compareHeld = false;
  }

  onMount(() => {
    // Focus moves to the editor heading on entry (WCAG 2.4.3).
    document.querySelector<HTMLElement>('[data-route-heading]')?.focus({ preventScroll: true });
    return () => clearTimeout(sayTimer);
  });

  // ---- derived view state ------------------------------------------------------------------------------------------------
  const noItems = $derived(banner.kind === 'noItems');
  const failedBanner = $derived(!!item && !noItems && !split && needsDrawCrop(item, store.strictness));
  const canEdit = $derived(!!item && item.status === 'ready' && !!edit && !failedBanner && !!sel);
  const reasons = $derived(sel?.confidence?.reasons ?? item?.confidence?.reasons ?? []);
  const headline = $derived(item ? reasonLine(item, tier) : null);
  const statusWord = $derived.by(() => {
    if (!cls) return '';
    if (decision === 'accepted') return 'accepted';
    if (decision === 'skipped') return 'skipped';
    return cls.needs ? 'needs review' : tier;
  });
  const srcUrl = $derived(item ? imageUrl('src', item.id, item.gen) : '');
  const resultUrl = $derived(item ? (sel ? cropImageUrl('result', item.id, sel.id, sel.renderKey) : imageUrl('result', item.id, item.gen)) : '');
  const aspect = $derived(item && item.height > 0 ? item.width / item.height : 0.75);
  const showLayer = $derived(!!item && hasItemLayer(item) && !failedBanner);
  const stageCrops = $derived<StageCrop[]>(
    crops
      .filter((c) => c.edit)
      .map((c) => ({
        id: c.id,
        order: c.order,
        quad: (draft && draft.crop === c.id ? draft.edit.quad : c.edit!.quad) as Quad,
        include: c.include,
        band: c.band,
      })),
  );
  const savedNames = $derived(item?.saved ? (item.saved.outputs.length ? item.saved.outputs : [item.saved.output]) : []);
  const accepted = $derived(split ? !!item?.split?.accepted : decision === 'accepted');
  const canSave = $derived(!!item && item.status === 'ready' && gate.replace !== 'no-crop' && !store.saving && inflight === 0);
</script>

<svelte:window onkeydown={onKeydown} onkeyup={onKeyup} />

{#if !item}
  <div class="page missing">
    <h1 data-route-heading tabindex="-1">{S.editor.notFound}</h1>
    <a class="btn btn-primary" href={href('/grid')}>{S.nav.backToGrid}</a>
  </div>
{:else}
  <div class="editor">
    <h1 class="sr-only" data-route-heading tabindex="-1">{S.editor.heading(item.name, position, queue.length, statusWord)}</h1>

    <header class="bar">
      <a class="btn back" href={href('/grid')} onclick={() => (store.lastOpened = id)}><Icon name="back" size={18} /> {S.nav.backToGrid}</a>
      <span class="name mono" title={item.name}>{item.name}</span>
      {#if position > 0}<span class="pos">{S.editor.positionOf(position, queue.length)}</span>{/if}
      <span class="chips">
        {#if item.status === 'ready'}
          <TierBadge tier={tier} label={headline ? `${S.tier[tier === 'analysing' ? 'check' : tier]}: ${headline}` : undefined} />
          {#if !split}
            {#each reasons.slice(1) as r, i (i)}
              <span class="badge neutral">{holdTitle(r)}</span>
            {/each}
          {/if}
        {/if}
        {#if item.edited}<span class="badge accent">{S.editor.editedChip}</span>{/if}
        {#if decision === 'skipped'}<span class="badge neutral">{S.editor.skippedChip}</span>{/if}
        {#if item.saved}<span class="badge good">{S.editor.savedChip}</span>{/if}
        {#if item.openOnly}<span class="badge neutral" title={noticeText(item.openOnly)}>{S.grid.openOnlyBadge}</span>{/if}
      </span>
      <span class="grow"></span>
      <button
        type="button"
        class="icon-btn"
        disabled={!item.canUndo}
        aria-label={item.canUndo ? `${S.editor.undo} ${item.undoLabel ?? ''}` : S.editor.nothingToUndo}
        title={item.canUndo ? `${S.editor.undo} ${item.undoLabel ?? ''}` : S.editor.nothingToUndo}
        onclick={doUndo}><Icon name="undo" /></button
      >
      <button
        type="button"
        class="icon-btn"
        disabled={!item.canRedo}
        aria-label={item.canRedo ? `${S.editor.redo} ${item.redoLabel ?? ''}` : S.editor.nothingToRedo}
        title={item.canRedo ? `${S.editor.redo} ${item.redoLabel ?? ''}` : S.editor.nothingToRedo}
        onclick={doRedo}><Icon name="redo" /></button
      >
      <button
        type="button"
        class="btn"
        aria-pressed={compare}
        title={S.editor.compareHint}
        disabled={!canEdit}
        onpointerdown={compareDown}
        onpointerup={compareUp}
        onpointercancel={() => (compareHeld = false)}
        onpointerleave={() => (compareHeld = false)}
        onclick={(e) => {
          if (e.detail === 0) compareLatched = !compareLatched; // keyboard activation
        }}><Icon name="compare" size={16} /> {S.editor.compare}</button
      >
      <button type="button" class="btn btn-primary" disabled={!canEdit && !accepted} aria-pressed={accepted} onclick={toggleAccepted}>
        {#if accepted}<Icon name="check2" size={16} stroke={2.5} /> {split ? S.split.accepted(item.split?.included ?? 0) : S.editor.accepted}{:else}{split ? S.split.accept : S.editor.accept}{/if}
      </button>
    </header>

    {#if banner.kind !== 'none'}
      <div class="bannerrow">
        <ScanBanner
          state={banner}
          onaccept={() => void acceptSplit()}
          onreview={reviewItems}
          onwithdraw={() => void store.unacceptScan(id)}
          ondraw={() => startAdd('draw')}
          ontreatasone={() => setPolicy('never')}
          onskip={skip}
        />
      </div>
    {/if}
    {#if item.openOnly}
      <div class="bannerrow">
        <div class="openonly" role="note" data-open-only>
          <Icon name="info" size={18} />
          <span><b>{S.save.openOnly}</b> {noticeText(item.openOnly)}</span>
        </div>
      </div>
    {/if}

    <div class="body">
      <div class="work">
        {#if item.status === 'analysing'}
          <div class="placeholder"><span class="skeleton fill"></span><p>{S.editor.analysing}</p></div>
        {:else if item.status === 'error'}
          <div class="placeholder"><p>{S.editor.loadError}</p></div>
        {:else}
          <CropStage
            bind:this={stage}
            src={srcUrl}
            name={item.name}
            fallbackAspect={aspect}
            crops={stageCrops}
            selectedId={selectedId}
            {tool}
            {mergeIds}
            {mergePreview}
            {cutPreview}
            showOverlay={!failedBanner && !compare && (canEdit || stageCrops.length > 0 || tool !== 'none')}
            {bandWord}
            onselect={selectCrop}
            onmenu={openMenu}
            onrestore={restoreCrop}
            onaddat={addAt}
            onaddbox={addBox}
            oncutdrag={cutDrag}
            onpreview={(q) => (previewQuad = q)}
            oncommit={commitQuad}
          >
            {#if prevId !== null}
              <button type="button" class="navbtn left" data-nostage aria-label={S.editor.previous} onclick={() => goTo(prevId!)}><Icon name="back" size={22} stroke={2} /></button>
            {:else}
              <span class="navbtn left off" aria-hidden="true"><Icon name="back" size={22} stroke={2} /></span>
            {/if}
            {#if nextId !== null}
              <button type="button" class="navbtn right" data-nostage aria-label={S.editor.next} onclick={() => goTo(nextId!)}><Icon name="next" size={22} stroke={2} /></button>
            {:else}
              <span class="navbtn right off" aria-hidden="true"><Icon name="next" size={22} stroke={2} /></span>
            {/if}

            {#if canEdit && shownEdit}
              <ResultInset {srcUrl} {resultUrl} edit={shownEdit} {busy} {compare} name={item.name} />
            {/if}

            {#if failedBanner}
              <div class="banner" role="alert" data-nostage>
                <div class="banner-head">
                  <Icon name="failed" size={22} stroke={1.9} />
                  <div>
                    <div class="banner-title">{S.editor.failedBannerTitle}</div>
                    <div class="banner-body">{S.editor.failedBannerBody}</div>
                  </div>
                </div>
                <div class="banner-actions">
                  <button type="button" class="btn btn-primary btn-lg" onclick={doDrawCrop}><Icon name="crop" size={18} stroke={1.9} /> {S.editor.drawCrop}</button>
                  <button type="button" class="btn btn-lg" onclick={skip}>{S.editor.skip} <span class="kbd mono">X</span></button>
                  <span class="banner-note">{S.editor.failedDrawNote}</span>
                </div>
              </div>
            {/if}
          </CropStage>

          {#if showLayer || noItems}
            <ItemDock
              {tool}
              count={included.length}
              selectedOrder={sel?.order ?? null}
              canMoveEarlier={!!sel && !!item && moveTarget(item, sel.id, -1) !== null}
              canMoveLater={!!sel && !!item && moveTarget(item, sel.id, 1) !== null}
              canRemove={!!sel}
              canCut={!!sel}
              canMerge={included.length >= 2}
              manualOrder={item.split?.orderMode === 'manual'}
              mergeCount={mergeIds.length}
              {cut}
              cutProblem={cutIssue}
              busy={inflight > 0}
              onadd={startAdd}
              onmerge={() => startMerge()}
              onmergeconfirm={confirmMerge}
              oncut={startCut}
              oncutchange={(c) => (cut = c)}
              oncutconfirm={confirmCut}
              oncancel={cancelTool}
              onmove={(d) => sel && moveCrop(sel.id, d)}
              onremove={() => sel && removeCrop(sel.id)}
              onreading={useReading}
            />
            {#if crops.length > 0}
              <ChipBar
                {item}
                selectedId={selectedId}
                {mergeIds}
                mergeMode={tool === 'merge'}
                {bandWord}
                {reasonOf}
                onselect={(cid) => selectCrop(cid, false)}
                onmenu={openMenu}
                onreorder={reorderCrop}
                onmove={moveCrop}
                onrestore={restoreCrop}
              />
            {/if}
          {/if}

          <div class="rulerbar">
            <button type="button" class="cbtn" aria-label={S.editor.rotateLeft90} disabled={!canEdit} onclick={() => turn(-1)}><Icon name="rotateLeft" /> {S.editor.left90}</button>
            <button type="button" class="cbtn sq" aria-label={S.editor.nudgeLeft} disabled={!canEdit} onclick={() => edit && commitAngle(edit.fineDeg - 0.1, S.editor.labels.rotate, 'key')}>−0.1</button>
            <Ruler value={edit?.fineDeg ?? 0} auto={sel?.autoEdit?.fineDeg ?? null} disabled={!canEdit} oncommit={(v) => commitAngle(v)} onlive={(v) => (liveAngle = v)} />
            <button type="button" class="cbtn sq" aria-label={S.editor.nudgeRight} disabled={!canEdit} onclick={() => edit && commitAngle(edit.fineDeg + 0.1, S.editor.labels.rotate, 'key')}>+0.1</button>
            <button type="button" class="cbtn" aria-label={S.editor.rotateRight90} disabled={!canEdit} onclick={() => turn(1)}>{S.editor.right90} <Icon name="rotateRight" /></button>
            <button type="button" class="cbtn auto" title={S.editor.autoAngle} disabled={!canEdit} onclick={() => commitAngle(sel?.autoEdit?.fineDeg ?? 0, S.editor.labels.autoAngle)}>{S.editor.auto}</button>
          </div>
        {/if}
      </div>

      <aside class="inspector" aria-label="Inspector">
        <section>
          <div class="sec-head">
            <h2>{multi && sel ? S.items.selectedOf(sel.order, included.length) : S.editor.cropHeading}</h2>
            <span class="note">{sel ? (sel.edited ? S.editor.outlineEdited : S.editor.outlineAuto) : item.edited ? S.editor.outlineEdited : S.editor.outlineAuto}</span>
          </div>
          {#if edit && !failedBanner}
            <div class="corners">
              <span class="cg-title">{S.editor.cornerPositions}</span>
              <div class="grid">
                {#each edit.quad as p, i (i)}
                  <span class="cn">{S.editor.cornersShort[i]}</span>
                  <input
                    class="input mono"
                    type="number"
                    step="0.1"
                    min="0"
                    max="100"
                    disabled={!canEdit}
                    aria-label={S.editor.cornerInput(S.editor.corners[i], 'x')}
                    value={toPercent(previewQuad?.[i].x ?? p.x)}
                    onchange={(e) => setCorner(i, 'x', e.currentTarget.value)}
                  />
                  <input
                    class="input mono"
                    type="number"
                    step="0.1"
                    min="0"
                    max="100"
                    disabled={!canEdit}
                    aria-label={S.editor.cornerInput(S.editor.corners[i], 'y')}
                    value={toPercent(previewQuad?.[i].y ?? p.y)}
                    onchange={(e) => setCorner(i, 'y', e.currentTarget.value)}
                  />
                {/each}
              </div>
            </div>
          {/if}
          <div class="btn-row">
            {#if multi && sel}
              <button type="button" class="btn" disabled={!sel.autoEdit} title={sel.autoEdit ? S.items.resetHelp : S.items.reviewed} onclick={() => resetCrop(sel.id)}>{S.items.resetToAuto}</button>
              <button type="button" class="btn" onclick={flip}>{S.items.flip}</button>
            {:else}
              <button type="button" class="btn" disabled={!item.autoEdit || failedBanner} onclick={doReset}>{S.editor.resetToAuto}</button>
            {/if}
            {#if failedBanner}
              <button type="button" class="btn btn-primary" onclick={doDrawCrop}><Icon name="crop" size={16} stroke={1.9} /> {S.editor.drawCrop}</button>
            {/if}
          </div>
        </section>

        <section>
          <h2>{multi && sel ? S.items.why(sel.order) : S.editor.whyHeading}</h2>
          {#if reasons.length > 0}
            <ul class="why">
              {#each reasons as r, i (i)}
                <li>
                  <div class="why-title">{holdTitle(r)}</div>
                  <div class="why-cause">{holdCause(r)}</div>
                  <div class="why-action">{holdAction(r)}</div>
                </li>
              {/each}
            </ul>
          {:else if sel && sel.origin !== 'auto'}
            <p class="plain">{S.items.reviewed}</p>
          {:else if tier === 'check' || tier === 'failed'}
            <p class="plain">{headline}</p>
          {:else}
            <p class="plain">{S.editor.whyNone}</p>
          {/if}
        </section>

        {#if item.status === 'ready' && item.split}
          <section data-items-settings>
            <h2>{S.items.itemSettings}</h2>
            <div class="seg wrap" role="group" aria-label={S.items.policyLabel}>
              <button type="button" aria-pressed={item.split.policy === 'auto'} onclick={() => setPolicy('auto')}>{S.items.policyAuto}</button>
              <button type="button" aria-pressed={item.split.policy === 'never'} onclick={() => setPolicy('never')}>{S.items.policyNever}</button>
              <button type="button" aria-pressed={item.split.policy === 'always'} onclick={() => setPolicy('always')}>{S.items.policyAlways}</button>
            </div>
            <label class="field">
              {S.items.profileLabel}
              <select class="select" value={item.split.profile} onchange={(e) => setProfile(e.currentTarget.value as SplitProfile)}>
                <option value="photos">{S.items.profilePhotos}</option>
                <option value="receipts">{S.items.profileReceipts}</option>
              </select>
            </label>
            <div class="order-note">{item.split.orderMode === 'manual' ? S.items.orderManual : S.items.orderReading}</div>
          </section>
        {/if}

        {#if item.status === 'ready'}
          <section class="savebox" data-save-box>
            <h2>{S.save.heading}</h2>
            {#if planned.length > 0}
              <div class="names" title={planned.join(', ')}>
                <span class="nl">{S.save.willSave('')}</span>
                <span class="mono">{nameSummary(planned)}</span>
              </div>
            {/if}
            <div class="btn-row">
              <button type="button" class="btn" disabled={!canSave} onclick={() => requestSave('copy')} data-save-copy>{S.save.saveAsCopy}</button>
              <button type="button" class="btn btn-primary" disabled={!canSave || gate.replace !== 'ok'} onclick={() => requestSave('replace')} data-save-replace>{S.save.replace}</button>
            </div>
            {#if gate.replace === 'accept-first'}
              <div class="gate" data-gate="accept-first">
                {S.save.acceptFirst}
                <button type="button" class="btn btn-sm" onclick={() => void acceptSplit()}>{S.split.accept}</button>
              </div>
            {:else if gate.replace === 'open-only'}
              <div class="gate" data-gate="open-only">{S.save.openOnly} {noticeText(gate.reason)}</div>
            {:else}
              <div class="sub">{store.settings.saveAsCopy ? S.save.copyNote : S.save.replaceNote}</div>
            {/if}
            {#if lastSave}
              {@const o = lastSave.outcome}
              <div class="result {o.ok ? 'ok' : o.error === 'HELD_FOR_REVIEW' || o.error === 'NOT_REPLACEABLE' ? 'warn' : 'bad'}" role="status" data-save-result>
                {#if o.ok}
                  <div class="rt"><b>{o.saved?.copy ? S.save.savedCopies(nameSummary(savedNames)) : S.save.savedAs(nameSummary(savedNames))}</b></div>
                  {#if o.saved?.backupId && split}<div>{S.save.movedToBackups}</div>{/if}
                  {#if lastSave.collision}<div>{S.save.collision(lastSave.collision.wanted, lastSave.collision.got)}</div>{/if}
                  {#each o.notes as n (n)}<div>{errorMessage(n)}</div>{/each}
                  {#each o.notices as n (n)}<div>{noticeText(n)}</div>{/each}
                {:else}
                  <div class="rt"><b>{o.error === 'HELD_FOR_REVIEW' ? S.save.held : S.save.failed('')}</b></div>
                  <div>{errorMessage(o.error)}</div>
                  {#each o.notices as n (n)}<div>{noticeText(n)}</div>{/each}
                {/if}
              </div>
            {/if}
          </section>
        {/if}

        {#if item.saved}
          <section class="savedline">
            <div class="sv">{S.editor.savedState(nameSummary(savedNames), item.saved.copy)}</div>
            {#if item.dirtySinceSave}<div class="sv-note">{S.editor.dirtySinceSave}</div>{/if}
            {#if item.saved.backupId}
              <a class="btn btn-sm" href={href('/backups')}>{S.editor.restoreOriginal}</a>
            {/if}
          </section>
        {/if}

        <div class="foot-note"><Icon name="info" size={16} /> {S.editor.editingFromOriginal}</div>
      </aside>
    </div>

    <div class="actions">
      <button type="button" class="btn btn-lg" disabled={item.status !== 'ready'} aria-pressed={decision === 'skipped'} onclick={skip}>
        {decision === 'skipped' ? S.editor.unskip : S.editor.skip} <span class="kbd mono">X</span>
      </button>
      <button type="button" class="btn btn-lg" disabled={!canEdit || !item.autoEdit} onclick={doReset}>{multi ? S.items.resetAll : S.editor.resetToAuto}</button>
      <span class="grow"></span>
      <span class="help">{multi ? S.items.chipHelp : S.editor.help}</span>
      <button type="button" class="btn btn-primary btn-lg" disabled={!canEdit} onclick={() => void acceptAndNext()}>
        {split ? S.split.acceptAndNext : S.editor.acceptAndNext} <Icon name="next" size={18} stroke={2} />
      </button>
    </div>
  </div>

  {#if menu}
    <Menu entries={menuEntries} at={menu} label={S.items.menu} onclose={closeMenu} />
  {/if}

  <FirstWriteSheet
    open={sheetOpen}
    count={1}
    copy={store.settings.saveAsCopy}
    days={store.settings.retentionDays}
    onchoose={chooseMode}
    oncancel={() => (sheetOpen = false)}
  />
  <FirstSplitSheet
    open={splitSheetOpen}
    count={planned.length}
    first={planned[0] ?? ''}
    onreplace={() => {
      splitSheetOpen = false;
      store.ackFirstSplit();
      void doSave('replace');
    }}
    onkeep={() => {
      splitSheetOpen = false;
      store.ackFirstSplit();
      void doSave('copy');
    }}
    oncancel={() => (splitSheetOpen = false)}
  />
{/if}

<style>
  .editor {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    /* On a short window the whole editor scrolls rather than squeezing the picture to nothing. */
    overflow-y: auto;
  }

  .missing {
    align-items: center;
    justify-content: center;
    gap: 16px;
  }

  .bar {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px 10px;
    padding: 8px 16px;
    min-height: 56px;
    background: var(--surface);
    border-bottom: 1px solid var(--line);
  }

  .bannerrow {
    flex-shrink: 0;
    padding: 8px 16px 0;
    background: var(--bg);
  }

  .openonly {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 14px;
    background: var(--info-bg);
    border: 1px solid var(--info-line);
    border-radius: 12px;
    color: var(--info-text);
    font-size: 13px;
    line-height: 1.4;
  }

  .back {
    border-color: transparent;
    background: transparent;
    padding: 0 12px 0 8px;
    gap: 4px;
  }

  .name {
    font-weight: 500;
    font-size: 14px;
  }

  .pos {
    font-size: 13px;
    color: var(--text-3);
  }

  .chips {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }

  .grow {
    flex: 1;
  }

  .body {
    /* A basis of 0 keeps the long inspector from stretching the picture; the floor keeps the picture usable. */
    flex: 1 1 0;
    min-height: 540px;
    display: flex;
  }

  .work {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--canvas);
  }

  .placeholder {
    flex: 1;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--canvas-text-2);
  }

  .placeholder .fill {
    position: absolute;
    inset: 24px;
    border-radius: 12px;
    opacity: 0.4;
  }

  .placeholder p {
    position: relative;
  }

  .navbtn {
    position: absolute;
    top: 36%;
    width: 48px;
    height: 48px;
    margin-top: -24px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    border: 1px solid rgba(255, 255, 255, 0.3);
    background: rgba(255, 255, 255, 0.12);
    color: #fff;
  }

  .navbtn:hover:not(.off) {
    background: rgba(255, 255, 255, 0.22);
  }

  .navbtn:focus-visible {
    outline-color: #ffffff;
  }

  .navbtn.off {
    background: rgba(255, 255, 255, 0.05);
    border-color: rgba(255, 255, 255, 0.12);
    color: rgba(255, 255, 255, 0.35);
  }

  .navbtn.left {
    left: 12px;
  }

  .navbtn.right {
    right: 12px;
  }

  .banner {
    position: absolute;
    left: 50%;
    bottom: 24px;
    transform: translateX(-50%);
    width: min(820px, calc(100% - 32px));
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    background: var(--check-bg);
    border: 1px solid var(--check-line);
    border-radius: 14px;
    color: var(--check-text-strong);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.4);
    cursor: default;
  }

  .banner-head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    color: var(--check-fg);
  }

  .banner-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--check-text-strong);
  }

  .banner-body {
    margin-top: 4px;
    font-size: 13px;
    line-height: 1.45;
    color: var(--check-text-strong);
  }

  .banner-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
  }

  .banner-note {
    font-size: 12px;
    color: var(--check-text-strong);
  }

  .kbd {
    margin-left: 4px;
    font-size: 12px;
    color: var(--text-3);
  }

  .rulerbar {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    background: var(--canvas-bar);
    border-top: 1px solid var(--canvas-line);
  }

  .cbtn {
    min-height: max(44px, var(--ctl-h));
    padding: 0 12px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: var(--canvas-btn);
    border: 1px solid var(--canvas-btn-line);
    border-radius: 8px;
    color: #fff;
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
  }

  .cbtn.sq {
    width: 48px;
    padding: 0;
    justify-content: center;
    font-size: 12px;
  }

  .cbtn.auto {
    background: transparent;
    border-color: var(--canvas-accent);
    color: #bfd0ff;
    font-weight: 600;
  }

  .cbtn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.18);
  }

  .cbtn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .cbtn:focus-visible {
    outline-color: #ffffff;
  }

  .inspector {
    width: 320px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    overflow: auto;
    background: var(--surface);
    border-left: 1px solid var(--line);
  }

  .inspector section {
    padding: 16px 18px 14px;
    border-bottom: 1px solid var(--line-soft);
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  h2 {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.06em;
    color: var(--text-2);
  }

  .note {
    font-size: 12px;
    color: var(--text-3);
  }

  .cg-title {
    font-size: 13px;
    font-weight: 500;
  }

  .corners {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .grid {
    display: grid;
    grid-template-columns: 34px repeat(2, minmax(0, 1fr));
    gap: 6px;
    align-items: center;
  }

  .cn {
    font-size: 12px;
    color: var(--text-2);
  }

  .grid .input {
    width: 100%;
    min-width: 0;
    min-height: var(--ctl-h-sm);
    padding: 0 6px;
  }

  .btn-row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .why {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .why-title {
    font-weight: 600;
    font-size: 13px;
  }

  .why-cause {
    margin-top: 2px;
    font-size: 13px;
    color: var(--text-2);
    line-height: 1.4;
  }

  .why-action {
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-3);
  }

  .plain {
    margin: 0;
    font-size: 13px;
    color: var(--text-2);
  }

  .seg.wrap {
    flex-wrap: wrap;
    width: 100%;
  }

  .seg.wrap button {
    flex: 1 1 auto;
    justify-content: center;
    white-space: normal;
    text-align: center;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
    color: var(--text-2);
  }

  .order-note,
  .sub {
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.4;
  }

  .names {
    font-size: 13px;
    line-height: 1.4;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }

  .names .mono {
    color: var(--text);
    font-size: 12px;
  }

  .gate {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: 8px 10px;
    background: var(--check-bg);
    border: 1px solid var(--check-line);
    border-radius: 8px;
    color: var(--check-text-strong);
    font-size: 12px;
    line-height: 1.4;
  }

  .result {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 10px;
    border-radius: 8px;
    font-size: 12px;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }

  .result.ok {
    background: var(--good-bg);
    border: 1px solid var(--good-line);
    color: var(--good-fg);
  }

  .result.warn {
    background: var(--check-bg);
    border: 1px solid var(--check-line);
    color: var(--check-text-strong);
  }

  .result.bad {
    background: var(--fail-bg);
    border: 1px solid var(--fail-line);
    color: var(--fail-fg);
  }

  .savedline .sv {
    font-size: 13px;
    font-weight: 500;
    color: var(--good-fg);
    overflow-wrap: anywhere;
  }

  .savedline .sv-note {
    font-size: 12px;
    color: var(--check-fg);
  }

  .savedline .btn {
    align-self: flex-start;
  }

  .foot-note {
    margin: auto 18px 14px;
    padding-top: 12px;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.4;
  }

  .actions {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    min-height: 64px;
    padding: 8px 16px;
    background: var(--surface);
    border-top: 1px solid var(--line);
  }

  .help {
    font-size: 12px;
    color: var(--text-3);
    max-width: 420px;
    text-align: right;
  }

  @media (max-width: 900px) {
    .body {
      flex: none;
      flex-direction: column;
      min-height: 0;
    }

    .work {
      min-height: 62vh;
    }

    .inspector {
      width: auto;
      border-left: 0;
      border-top: 1px solid var(--line);
      overflow: visible;
    }

    .help {
      display: none;
    }

    .rulerbar {
      flex-wrap: wrap;
    }
  }
</style>
