<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // Convert formats: add files, choose what to make of them, see what will happen, run, and check the results.
  // Idle: the file list on the left, the options and the preview on the right (one column on a phone).
  // Running and done: a progress or summary card above the per-file results. Words come from strings.ts.
  import { onMount, tick } from 'svelte';
  import Icon from '../lib/components/Icon.svelte';
  import RestoreDialog from '../lib/components/RestoreDialog.svelte';
  import Switch from '../lib/components/Switch.svelte';
  import { convert } from '../lib/convert.svelte.ts';
  import { chipNotices, CUSTOM_QUALITY_START, TARGETS, usesQuality } from '../lib/convert.ts';
  import { formatBytes } from '../lib/format.ts';
  import { href } from '../lib/router.svelte.ts';
  import { store } from '../lib/store.svelte.ts';
  import { convertCannotText, convertNoticeText, formatName, S } from '../lib/strings.ts';
  import type { ConvertFileResult } from '../lib/types.ts';

  let dragging = $state(false);

  // The file list is one tab stop: arrow keys walk the remove buttons (a long list must not cost a hundred Tab presses).
  let listEl = $state<HTMLUListElement | null>(null);
  let activeRow = $state(0);
  const rowTab = (i: number) => (i === Math.min(activeRow, convert.files.length - 1) ? 0 : -1);

  $effect(() => {
    const el = listEl;
    if (!el) return;
    el.addEventListener('keydown', onListKey);
    return () => el.removeEventListener('keydown', onListKey);
  });

  function onListKey(e: KeyboardEvent): void {
    const last = convert.files.length - 1;
    const at = Math.min(activeRow, last);
    const next = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: last }[e.key];
    if (next === undefined) return;
    e.preventDefault();
    activeRow = Math.max(0, Math.min(last, next));
    listEl?.querySelectorAll<HTMLButtonElement>('button')[activeRow]?.focus();
  }

  onMount(() => {
    void convert.enter();
    return () => convert.leave();
  });

  // Focus follows the phase: the progress or summary card when a run starts and ends, the file list when it is over.
  let lastPhase = convert.phase;
  $effect(() => {
    const p = convert.phase;
    if (p === lastPhase) return;
    lastPhase = p;
    void tick().then(() => document.getElementById(p === 'idle' ? 'files-h' : 'run-h')?.focus({ preventScroll: false }));
  });

  const o = $derived(convert.options);
  const days = $derived(store.settings.retentionDays);
  const targetNames = { jpg: S.convert.targetJpg, png: S.convert.targetPng, tiff: S.convert.targetTiff, webp: S.convert.targetWebp, pdf: S.convert.targetPdf } as const;
  const target = $derived(targetNames[o.target]);
  const qualityOn = $derived(usesQuality(o.target));
  const fixed = $derived(o.quality.kind === 'fixed' ? o.quality.value : null);
  const preset = $derived(o.quality.kind === 'preset' ? o.quality.preset : null);
  const n = $derived(convert.files.length);
  const p = $derived(convert.preview);
  const pct = $derived(convert.total > 0 ? Math.round((convert.done / convert.total) * 100) : 0);
  const running = $derived(convert.phase === 'running');
  const cs = $derived(convert.counts);
  const anyBackup = $derived(convert.results.some((r) => r.backupId && r.status === 'converted'));

  const presets = [
    { id: 'small', label: S.convert.qualitySmall, note: S.convert.qualitySmallNote },
    { id: 'balanced', label: S.convert.qualityBalanced, note: S.convert.qualityBalancedNote },
    { id: 'best', label: S.convert.qualityBest, note: S.convert.qualityBestNote },
  ] as const;

  const why = $derived(
    {
      files: S.convert.whyNoFiles,
      checking: S.convert.whyChecking,
      nothing: S.convert.whyNothing,
      folder: S.convert.whyNeedFolder,
    }[convert.why ?? 'files'],
  );

  const originalsLine = $derived(
    o.placement === 'replace' ? S.convert.originalsReplace(days) : o.placement === 'keep' ? S.convert.originalsKeep : S.convert.originalsFolder(p?.folder ?? null),
  );

  const statusWord = (r: ConvertFileResult) => ({ converted: S.convert.statusConverted, skipped: S.convert.statusSkipped, failed: S.convert.statusFailed })[r.status];
  const statusIcon = (r: ConvertFileResult) => ({ converted: 'good', skipped: 'skipped', failed: 'failed' } as const)[r.status];
  const statusClass = (r: ConvertFileResult) => ({ converted: 'good', skipped: 'neutral', failed: 'failed' })[r.status];

  const doneTitle = $derived(convert.summary?.cancelled || convert.interrupted ? S.convert.stoppedTitle : S.convert.doneTitle);

  function onCustomNumber(e: Event & { currentTarget: HTMLInputElement }): void {
    convert.setCustomQuality(Number(e.currentTarget.value));
    e.currentTarget.value = String(convert.options.quality.kind === 'fixed' ? convert.options.quality.value : CUSTOM_QUALITY_START);
  }
</script>

<div class="page">
  <header class="top">
    <a class="icon-btn" href={href('/')} aria-label={S.nav.backToHome}><Icon name="back" size={20} /></a>
    <div class="titles">
      <h1 data-route-heading tabindex="-1">{S.convert.title}</h1>
      <p>{S.convert.intro}</p>
    </div>
  </header>

  <div class="page-scroll">
    {#if convert.phase !== 'idle'}
      <!-- ---------------------------------------------------------------- running and results -->
      <div class="body">
        <section class="card runbar" aria-labelledby="run-h">
          {#if running}
            <div class="runhead">
              <h2 id="run-h" tabindex="-1">{S.convert.running}</h2>
              <span class="runof">{S.convert.runningOf(convert.done, convert.total)}</span>
              <button type="button" class="btn push" disabled={convert.cancelling} onclick={() => convert.cancel()}>{S.convert.cancel}</button>
            </div>
            <div class="barrow">
              <div
                class="bar"
                role="progressbar"
                aria-label={S.convert.progressLabel}
                aria-valuemin="0"
                aria-valuemax={convert.total}
                aria-valuenow={convert.done}
                aria-valuetext={S.convert.runningOf(convert.done, convert.total)}
              >
                <div class="fill" style:width="{pct}%"></div>
              </div>
              <span class="pct" aria-hidden="true">{pct}%</span>
            </div>
            {#if convert.cancelling}<p class="note">{S.convert.cancelling}</p>{/if}
          {:else}
            <div class="runhead">
              <h2 id="run-h" tabindex="-1">{doneTitle}</h2>
            </div>
            <p class="counts">{S.convert.counts(cs.converted, cs.skipped, cs.failed)}</p>
            {#if cs.converted > 0}<p class="sizes">{S.convert.sizeSummary(formatBytes(cs.bytesIn), formatBytes(cs.bytesOut))}</p>{/if}
            {#if convert.interrupted}<p class="note warn">{S.convert.runFailed}</p>{:else if convert.summary?.cancelled}<p class="note">{S.convert.stoppedNote}</p>{/if}
            {#if anyBackup}<p class="note">{S.convert.originalsInBackups}</p>{/if}
            <div class="runbtns">
              <button type="button" class="btn btn-primary btn-lg" onclick={() => convert.more()}>{S.convert.convertMore}</button>
              {#if cs.converted > 0}
                <button type="button" class="btn btn-lg" onclick={() => convert.openFolder(null)}><Icon name="external" size={18} /> {S.convert.openFolder}</button>
              {/if}
              {#if anyBackup}
                <a class="btn btn-lg" href={href('/backups')}><Icon name="backups" size={18} /> {S.convert.openBackups}</a>
              {/if}
            </div>
          {/if}
        </section>

        <section aria-label={S.convert.resultsLabel}>
          <ul class="results">
            {#each convert.results as r (r.id)}
              {@const chips = chipNotices(r)}
              <li class="res">
                <div class="rtop">
                  <span class="rname mono">{r.name}</span>
                  <span class="badge {statusClass(r)}"><Icon name={statusIcon(r)} size={14} stroke={2} /> {statusWord(r)}</span>
                </div>
                {#if r.status === 'converted'}
                  <div class="rmeta">
                    {#if r.output}<span>{S.convert.outputName(r.output)}</span>{/if}
                    <span>{S.convert.sizeChange(formatBytes(r.bytesIn), formatBytes(r.bytesOut))}</span>
                    {#if r.width && r.height}<span>{S.convert.dimensions(r.width, r.height)}</span>{/if}
                  </div>
                {:else}
                  <div class="reason">{convert.reasonOf(r)}</div>
                {/if}
                {#if chips.length > 0}
                  <ul class="chips" aria-label={r.name}>
                    {#each chips as c (c)}
                      <li class="chip"><Icon name="info" size={14} /> {convertNoticeText(c, r.status)}</li>
                    {/each}
                  </ul>
                {/if}
                {#if r.status === 'converted'}
                  <div class="ractions">
                    {#if r.backupId}
                      {#if convert.restored.has(r.id)}
                        <span class="badge good"><Icon name="good" size={14} stroke={2} /> {S.convert.restoredBadge}</span>
                      {:else}
                        <button
                          type="button"
                          class="btn btn-sm"
                          aria-label={S.convert.restoreOriginalOf(r.name)}
                          disabled={convert.restoring.has(r.id)}
                          onclick={() => convert.askRestore(r)}><Icon name="undo" size={16} /> {S.convert.restoreOriginal}</button
                        >
                      {/if}
                    {/if}
                    <button type="button" class="btn btn-sm" aria-label={S.convert.openFolderFor(r.name)} onclick={() => convert.openFolder(r.id)}>
                      <Icon name="external" size={16} /> {S.convert.openFolder}
                    </button>
                  </div>
                  {#if convert.asking.has(r.id) && !convert.restored.has(r.id)}
                    {@const action = convert.asking.get(r.id) ?? 'keep'}
                    <div class="ask" role="group" aria-label={S.convert.changedSince}>
                      <span>{S.convert.changedSince}</span>
                      <button type="button" class="btn btn-sm btn-primary" disabled={convert.restoring.has(r.id)} onclick={() => convert.restore(r, action, 'as_copy')}>{S.convert.restoreAsCopy}</button>
                      <button type="button" class="btn btn-sm" disabled={convert.restoring.has(r.id)} onclick={() => convert.restore(r, action, 'replace_anyway')}>{S.convert.replaceAnyway}</button>
                    </div>
                  {/if}
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      </div>
    {:else}
      <!-- ---------------------------------------------------------------- files, options, preview -->
      <div class="cols">
        <div class="left">
          <section
            class="card files"
            class:dragging
            aria-labelledby="files-h"
            ondragenter={() => (dragging = true)}
            ondragover={() => (dragging = true)}
            ondragleave={() => (dragging = false)}
            ondrop={() => (dragging = false)}
          >
            <div class="fhead">
              <h2 id="files-h" tabindex="-1">{S.convert.filesHeading}{#if n > 0}<span class="count">{S.convert.fileCount(n)}</span>{/if}</h2>
              {#if n > 0}
                <button type="button" class="btn-link" onclick={() => convert.clear()}>{S.convert.clearAll}</button>
              {/if}
            </div>

            {#if n === 0}
              <div class="drop">
                <div class="drop-icon"><Icon name="upload" size={26} /></div>
                <div class="drop-title">{S.convert.dropTitle}</div>
                <div class="drop-sub">{S.convert.dropSub}</div>
                <div class="drop-formats mono">{S.convert.dropFormats}</div>
              </div>
            {:else}
              <ul class="flist" bind:this={listEl} aria-label={S.convert.listLabel} aria-describedby="flist-help">
                {#each convert.files as f, i (f.id)}
                  <li>
                    <span class="badge neutral fmt">{formatName(f.format)}</span>
                    <span class="fname mono">{f.name}</span>
                    <span class="fsize">{formatBytes(f.bytes)}</span>
                    <button type="button" class="icon-btn" aria-label={S.convert.remove(f.name)} tabindex={rowTab(i)} onfocus={() => (activeRow = i)} onclick={() => convert.remove(f.id)}>
                      <Icon name="close" size={16} />
                    </button>
                  </li>
                {/each}
              </ul>
              <p id="flist-help" class="sr-only">{S.convert.listKeys}</p>
              <p class="hint">{S.convert.dropMore}</p>
            {/if}

            <div class="buttons">
              <button type="button" class="btn btn-lg" class:btn-primary={n === 0} disabled={convert.adding} onclick={convert.pickFiles}>
                <Icon name="image" size={20} /> {S.convert.chooseFiles}
              </button>
              <button type="button" class="btn btn-lg" disabled={convert.adding} onclick={convert.pickFolder}>
                <Icon name="folder" size={20} /> {S.convert.chooseFolder}
              </button>
            </div>
            <label class="sub">
              <input type="checkbox" checked={store.includeSubfolders} onchange={(e) => store.setIncludeSubfolders(e.currentTarget.checked)} />
              {S.convert.subfolders}
            </label>
          </section>
        </div>

        <div class="right">
          <section class="card pad" aria-labelledby="opt-h">
            <h2 id="opt-h">{S.convert.optionsHeading}</h2>

            <fieldset class="group">
              <legend>{S.convert.targetLabel}</legend>
              <div class="seg wrap">
                {#each TARGETS as t (t)}
                  <button type="button" aria-pressed={o.target === t} onclick={() => convert.setOptions({ target: t })} data-target={t}>{targetNames[t]}</button>
                {/each}
              </div>
              {#if o.target === 'tiff'}<p class="caption">{S.convert.tiffNote}</p>{/if}
            </fieldset>

            <fieldset class="group" disabled={!qualityOn}>
              <legend>{S.convert.qualityLabel}</legend>
              <div class="seg wrap">
                {#each presets as pr (pr.id)}
                  <button type="button" aria-pressed={preset === pr.id} onclick={() => convert.setPreset(pr.id)}>{pr.label}</button>
                {/each}
                <button type="button" aria-pressed={fixed !== null} onclick={() => convert.setCustomQuality(fixed ?? CUSTOM_QUALITY_START)}>{S.convert.qualityCustom}</button>
              </div>
              {#if !qualityOn}
                <p class="caption">{S.convert.losslessNoQuality(target)}</p>
              {:else if fixed !== null}
                <div class="custom">
                  <input
                    type="range"
                    min="1"
                    max="100"
                    step="1"
                    value={fixed}
                    aria-label={S.convert.qualityValue}
                    oninput={(e) => convert.setCustomQuality(Number(e.currentTarget.value))}
                  />
                  <input class="input num" type="number" min="1" max="100" step="1" inputmode="numeric" value={fixed} aria-label={S.convert.qualityNumber} onchange={onCustomNumber} />
                </div>
                <p class="caption">{S.convert.qualityCustomNote}</p>
              {:else}
                <p class="caption">{presets.find((x) => x.id === preset)?.note}</p>
              {/if}
            </fieldset>

            <fieldset class="group">
              <legend>{S.convert.colourLabel}</legend>
              <label class="opt" class:on={o.colour === 'srgb'}>
                <input type="radio" name="colour" checked={o.colour === 'srgb'} onchange={() => convert.setOptions({ colour: 'srgb' })} />
                <span><b>{S.convert.colourSrgb}</b><span class="d">{S.convert.colourSrgbNote}</span></span>
              </label>
              <label class="opt" class:on={o.colour === 'preserve'}>
                <input type="radio" name="colour" checked={o.colour === 'preserve'} onchange={() => convert.setOptions({ colour: 'preserve' })} />
                <span><b>{S.convert.colourPreserve}</b><span class="d">{S.convert.colourPreserveNote}</span></span>
              </label>
            </fieldset>

            <fieldset class="group">
              <legend>{S.convert.whereLabel}</legend>
              <label class="opt" class:on={o.placement === 'replace'}>
                <input type="radio" name="where" checked={o.placement === 'replace'} onchange={() => convert.setOptions({ placement: 'replace' })} />
                <span><b>{S.convert.whereReplace(days)}</b></span>
              </label>
              <label class="opt" class:on={o.placement === 'keep'}>
                <input type="radio" name="where" checked={o.placement === 'keep'} onchange={() => convert.setOptions({ placement: 'keep' })} />
                <span><b>{S.convert.whereKeep}</b></span>
              </label>
              <label class="opt" class:on={o.placement === 'folder'}>
                <input type="radio" name="where" checked={o.placement === 'folder'} onchange={() => convert.setOptions({ placement: 'folder' })} />
                <span><b>{S.convert.whereFolder}</b></span>
              </label>
              {#if o.placement === 'folder'}
                <div class="folderrow">
                  <button type="button" class="btn btn-sm" onclick={() => convert.pickOutputFolder()}><Icon name="folder" size={16} /> {p?.folder ? S.convert.changeOutput : S.convert.chooseOutput}</button>
                  <span class="foldername" class:none={!p?.folder}>{p?.folder ? S.convert.outputFolder(p.folder) : S.convert.outputFolderNone}</span>
                </div>
              {/if}
            </fieldset>

            <div class="swrow">
              <div>
                <div class="label">{S.convert.stripLocation}</div>
                <div class="caption">{S.convert.stripLocationNote}</div>
              </div>
              <Switch checked={o.stripLocation} label={S.convert.stripLocation} onchange={(v) => convert.setOptions({ stripLocation: v })} />
            </div>
            <div class="swrow">
              <div>
                <div class="label">{S.convert.reencode}</div>
                <div class="caption">{S.convert.reencodeNote(target)}</div>
              </div>
              <Switch checked={o.reencode} label={S.convert.reencode} onchange={(v) => convert.setOptions({ reencode: v })} />
            </div>
          </section>

          <section class="card pad" aria-labelledby="prev-h" aria-busy={convert.checking}>
            <h2 id="prev-h">{S.convert.previewHeading}</h2>
            {#if n === 0}
              <p class="caption nomargin">{S.convert.whyNoFiles}</p>
            {:else if convert.previewFailed && !p}
              <p class="warnline" role="status">{S.convert.previewFailed}</p>
            {:else if !p}
              <p class="caption nomargin" role="status">{S.convert.previewChecking}</p>
            {:else}
              <div class="counts-box" class:stale={convert.checking} aria-live="polite">
                <div class="big">{S.convert.willConvert(p.willConvert)}</div>
                {#if p.sameFormat > 0}<div class="line">{S.convert.sameFormat(p.sameFormat, target)}</div>{/if}
                {#if p.cannot > 0}<div class="line warn"><Icon name="check" size={16} /> {S.convert.cannot(p.cannot)}</div>{/if}
                {#if p.willConvert > 0}<div class="line dim">{S.convert.sizeToConvert(formatBytes(p.bytesToConvert))}</div>{/if}
              </div>

              {#if p.byFormat.length > 0}
                <div class="byformat">
                  <span class="lbl">{S.convert.byFormat}</span>
                  <ul class="fmts">
                    {#each p.byFormat as bf (bf.format)}
                      <li class="badge neutral">{S.convert.byFormatItem(formatName(bf.format), bf.count)}</li>
                    {/each}
                  </ul>
                </div>
              {/if}

              {#if p.heifUnavailable > 0}
                <div class="callout" role="note"><Icon name="info" size={18} /> <span>{S.convert.heifMissing(p.heifUnavailable)}</span></div>
              {/if}

              {#if p.cannotFiles.length > 0}
                <div class="cannot">
                  <h3>{S.convert.cannotHeading}</h3>
                  <ul>
                    {#each p.cannotFiles as c (c.id)}
                      <li>
                        <span class="mono cname">{c.name}</span>
                        <span class="badge neutral">{formatName(c.format)}</span>
                        <span class="creason">{convertCannotText(c.code)}</span>
                      </li>
                    {/each}
                  </ul>
                  {#if p.cannot > p.cannotFiles.length}<p class="caption nomargin">{S.convert.cannotMore(p.cannot - p.cannotFiles.length)}</p>{/if}
                </div>
              {/if}

              <p class="originals"><Icon name="shield" size={18} /> <span>{originalsLine}</span></p>
            {/if}
          </section>

          <div class="actionbar">
            <button type="button" class="btn btn-primary btn-lg go" disabled={!convert.canRun} aria-describedby={convert.canRun ? undefined : 'why-off'} onclick={() => convert.run()}>
              <Icon name="convert" size={20} /> {p && p.willConvert > 0 ? S.convert.convertN(p.willConvert) : S.convert.convertNone}
            </button>
            {#if !convert.canRun}<p id="why-off" class="why">{why}</p>{/if}
          </div>
        </div>
      </div>
    {/if}
  </div>
</div>

<RestoreDialog
  open={convert.ask !== null}
  title={convert.ask ? S.restoreDialog.title(convert.ask.result.name) : ''}
  files={convert.ask?.result.output ? [{ name: convert.ask.result.output, bytes: convert.ask.result.bytesOut, state: 'unchanged' }] : []}
  busy={convert.ask ? convert.restoring.has(convert.ask.result.id) : false}
  variant="convert"
  onrestore={(a) => convert.ask && void convert.restore(convert.ask.result, a)}
  oncancel={() => convert.cancelAsk()}
/>

<style>
  .top {
    flex-shrink: 0;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 18px 24px 10px;
  }

  .titles h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .titles p {
    margin: 2px 0 0;
    font-size: 13px;
    color: var(--text-3);
  }

  .cols {
    display: grid;
    grid-template-columns: minmax(0, 6fr) minmax(0, 5fr);
    gap: 24px;
    padding: 8px 24px 24px;
    align-items: start;
  }

  .left,
  .right {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-width: 0;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 8px 24px 24px;
    max-width: 960px;
  }

  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  h2[tabindex='-1']:focus {
    outline: none;
  }

  /* ---- files */
  .files {
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .files.dragging {
    border-color: var(--accent);
    background: var(--accent-tint);
  }

  .fhead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .count {
    margin-left: 8px;
    font-weight: 400;
    color: var(--text-3);
  }

  .drop {
    min-height: 200px;
    border: 2px dashed var(--switch-off);
    border-radius: 14px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 20px 14px;
    text-align: center;
  }

  .dragging .drop {
    border-color: var(--accent);
  }

  .drop-icon {
    width: 52px;
    height: 52px;
    border-radius: 50%;
    background: var(--accent-tint);
    color: var(--accent-text);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .drop-title {
    margin-top: 4px;
    font-size: 18px;
    font-weight: 600;
  }

  .drop-sub,
  .drop-formats {
    color: var(--text-3);
  }

  .drop-formats {
    font-size: 12px;
    margin-top: 2px;
  }

  .flist {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 380px;
    overflow: auto;
    border: 1px solid var(--line-soft);
    border-radius: 10px;
  }

  .flist li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 4px 4px 10px;
    min-height: var(--ctl-h);
    border-bottom: 1px solid var(--line-soft);
    content-visibility: auto;
    contain-intrinsic-size: auto 44px;
  }

  .flist li:last-child {
    border-bottom: 0;
  }

  .fmt {
    min-width: 52px;
    justify-content: center;
    padding: 0 8px;
  }

  .fname {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .fsize {
    flex-shrink: 0;
    font-size: 12px;
    color: var(--text-3);
  }

  .hint {
    margin: 0;
    font-size: 13px;
    color: var(--text-3);
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }

  .sub {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: var(--ctl-h-sm);
    color: var(--text-2);
    font-size: 14px;
  }

  /* ---- options */
  .pad {
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .group {
    margin: 0;
    padding: 0;
    border: 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .group:disabled {
    opacity: 0.55;
  }

  legend {
    padding: 0;
    margin-bottom: 6px;
    font-weight: 600;
    font-size: 13px;
  }

  .seg.wrap {
    flex-wrap: wrap;
    align-self: flex-start;
  }

  .seg {
    align-self: flex-start;
  }

  .caption {
    margin: 0;
    font-size: 13px;
    color: var(--text-3);
    line-height: 1.4;
  }

  .nomargin {
    margin: 0;
  }

  .custom {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .custom input[type='range'] {
    flex: 1;
    min-width: 0;
  }

  .num {
    width: 72px;
    text-align: center;
  }

  .opt {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    min-height: var(--ctl-h);
    border: 1px solid var(--line-strong);
    border-radius: 10px;
    cursor: pointer;
  }

  .opt.on {
    border-color: var(--accent);
    background: var(--accent-tint);
  }

  .opt input {
    margin-top: 1px;
    flex-shrink: 0;
  }

  .opt .d {
    display: block;
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-2);
  }

  .folderrow {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 12px;
    padding-left: 4px;
  }

  .foldername {
    font-size: 13px;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .foldername.none {
    font-weight: 400;
    color: var(--text-3);
  }

  .swrow {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding-top: 14px;
    border-top: 1px solid var(--line-soft);
  }

  .label {
    font-weight: 500;
  }

  /* ---- preview */
  .counts-box {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .counts-box.stale {
    opacity: 0.6;
  }

  .big {
    font-size: 20px;
    font-weight: 600;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
  }

  .line.warn {
    color: var(--check-fg);
    font-weight: 600;
  }

  .line.dim {
    color: var(--text-3);
    font-size: 13px;
  }

  .byformat {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .lbl {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
  }

  .fmts {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .callout {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 10px 12px;
    background: var(--check-bg);
    border: 1px solid var(--check-line);
    border-radius: 10px;
    color: var(--check-text-strong);
    font-size: 13px;
    line-height: 1.45;
  }

  .callout :global(svg) {
    margin-top: 1px;
  }

  .warnline {
    margin: 0;
    color: var(--check-text-strong);
    background: var(--check-bg);
    border: 1px solid var(--check-line);
    border-radius: 10px;
    padding: 10px 12px;
  }

  .cannot h3 {
    margin: 0 0 6px;
    font-size: 13px;
    font-weight: 600;
  }

  .cannot ul {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 220px;
    overflow: auto;
    border: 1px solid var(--line-soft);
    border-radius: 10px;
  }

  .cannot li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line-soft);
    font-size: 13px;
  }

  .cannot li:last-child {
    border-bottom: 0;
  }

  .cname {
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .creason {
    flex-basis: 100%;
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.4;
  }

  .originals {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    margin: 0;
    padding-top: 12px;
    border-top: 1px solid var(--line-soft);
    line-height: 1.45;
  }

  .originals :global(svg) {
    color: var(--good-fg);
    margin-top: 1px;
  }

  .actionbar {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .go {
    width: 100%;
  }

  .why {
    margin: 0;
    font-size: 13px;
    color: var(--text-2);
    text-align: center;
  }

  /* ---- run and results */
  .runbar {
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .runhead {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 14px;
  }

  .runhead h2 {
    font-size: 18px;
  }

  .runof {
    color: var(--text-2);
  }

  .push {
    margin-left: auto;
  }

  .barrow {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .bar {
    flex: 1;
    height: 12px;
    border-radius: 6px;
    background: var(--seg-bg);
    overflow: hidden;
    border: 1px solid var(--line);
  }

  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.2s ease;
  }

  .pct {
    min-width: 40px;
    text-align: right;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .counts {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }

  .sizes {
    margin: 0;
    color: var(--text-2);
  }

  .note {
    margin: 0;
    font-size: 13px;
    color: var(--text-2);
  }

  .note.warn {
    color: var(--check-text-strong);
  }

  .runbtns {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 4px;
  }

  .results {
    list-style: none;
    margin: 0;
    padding: 0;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 12px;
    overflow: hidden;
  }

  .res {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--line-soft);
    content-visibility: auto;
    contain-intrinsic-size: auto 90px;
  }

  .res:last-child {
    border-bottom: 0;
  }

  .rtop {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px 10px;
  }

  .rname {
    font-size: 13px;
    font-weight: 500;
    overflow-wrap: anywhere;
  }

  .rmeta {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 14px;
    font-size: 13px;
    color: var(--text-2);
  }

  .reason {
    font-size: 13px;
    color: var(--text-2);
    line-height: 1.4;
  }

  .chips {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .chip {
    display: inline-flex;
    align-items: flex-start;
    gap: 6px;
    padding: 4px 10px 4px 8px;
    background: var(--surface-2);
    border: 1px solid var(--line);
    border-radius: 8px;
    font-size: 12px;
    line-height: 1.4;
    color: var(--text-2);
  }

  .chip :global(svg) {
    margin-top: 1px;
  }

  .ractions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: 2px;
  }

  .ask {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 10px;
    font-size: 13px;
    color: var(--check-text-strong);
  }

  /* a wide row: the buttons sit to the right of the file, the note about a changed file under both */
  @media (min-width: 760px) {
    .res {
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      column-gap: 16px;
      align-items: start;
    }

    .res > * {
      grid-column: 1;
    }

    .res > .ractions {
      grid-column: 2;
      grid-row: 1 / span 3;
      align-self: center;
      margin-top: 0;
    }

    .res > .ask {
      grid-column: 1 / -1;
    }
  }

  @media (max-width: 900px) {
    .cols {
      grid-template-columns: minmax(0, 1fr);
      padding: 8px 16px 0;
      gap: 16px;
    }

    .top {
      padding: 12px 12px 8px;
    }

    .body {
      padding: 8px 16px 24px;
    }

    .files,
    .pad,
    .runbar {
      padding: 16px;
    }

    .flist {
      max-height: 300px;
    }

    /* the run button stays in reach while the options scroll */
    .actionbar {
      position: sticky;
      bottom: 0;
      z-index: 5;
      margin: 0 -16px;
      padding: 10px 16px 14px;
      background: var(--bg);
      border-top: 1px solid var(--line);
    }
  }
</style>
