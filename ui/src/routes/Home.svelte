<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import Icon from '../lib/components/Icon.svelte';
  import Switch from '../lib/components/Switch.svelte';
  import { href, navigate } from '../lib/router.svelte.ts';
  import { store } from '../lib/store.svelte.ts';
  import { S } from '../lib/strings.ts';

  let dragging = $state(false);

  const copy = $derived(store.settings.saveAsCopy);
  const days = $derived(store.settings.retentionDays);

  async function toggleCopy(next: boolean): Promise<void> {
    const ok = await store.updateSettings({ saveAsCopy: next });
    if (ok) store.announce(next ? S.toasts.copyModeOn : S.toasts.copyModeOff);
  }

  async function saveCopiesInstead(): Promise<void> {
    await store.updateSettings({ saveAsCopy: true });
    store.dismissHow(true);
  }
</script>

<div class="page">
  <header class="top">
    <h1 data-route-heading tabindex="-1">{S.home.title}</h1>
    <nav class="actions" aria-label="Sections">
      <a class="btn" href={href('/backups')}><Icon name="backups" /> {S.nav.backups}</a>
      <a class="btn" href={href('/settings')}><Icon name="settings" /> {S.nav.settings}</a>
    </nav>
  </header>

  <div class="page-scroll">
    <div class="cols">
      <div class="left">
        <!-- The window drop itself is handled by the shell; this zone explains it. -->
        <div class="drop" class:dragging role="group" aria-label={S.home.dropTitle} ondragenter={() => (dragging = true)} ondragover={() => (dragging = true)} ondragleave={() => (dragging = false)} ondrop={() => (dragging = false)}>
          <div class="drop-icon"><Icon name="upload" size={28} /></div>
          <div class="drop-title">{S.home.dropTitle}</div>
          <div class="drop-sub">{S.home.dropSub}</div>
          <div class="formats mono">{S.home.formats}</div>
          <div class="formats-note">{S.home.formatsNote}</div>
        </div>

        <div class="buttons">
          <button type="button" class="btn btn-primary btn-lg" disabled={store.opening} onclick={store.openFiles}>
            <Icon name="image" size={20} /> {S.home.openFiles}
          </button>
          <button type="button" class="btn btn-lg" disabled={store.opening} onclick={store.openFolder}>
            <Icon name="folder" size={20} /> {S.home.openFolder}
          </button>
        </div>
        <label class="sub">
          <input type="checkbox" checked={store.includeSubfolders} onchange={(e) => store.setIncludeSubfolders(e.currentTarget.checked)} />
          {S.home.subfolders}
        </label>
      </div>

      <div class="right">
        <section class="card pad" aria-labelledby="saving-h">
          <div class="row">
            <span class="shield"><Icon name="shield" /></span>
            <h2 id="saving-h">{S.home.saving}</h2>
            {#if store.howDismissed}
              <button type="button" class="btn-link push" onclick={() => store.dismissHow(false)}>{S.home.howSavingWorks}</button>
            {/if}
          </div>
          <div class="saving-line">{copy ? S.home.copyOnLine : S.home.replaceLine(days)}</div>
          <div class="copy-row">
            <div>
              <div class="label">{S.home.saveAsCopy}</div>
              <div class="caption">{copy ? S.home.copyOnCaption : S.home.copyOffCaption}</div>
            </div>
            <Switch checked={copy} label={S.home.saveAsCopy} onchange={toggleCopy} />
          </div>
        </section>

        {#if !store.howDismissed}
          <section class="info-card pad" aria-labelledby="how-h">
            <h2 id="how-h">{S.home.howSavingWorks}</h2>
            <p>{S.home.howBody(days)}</p>
            <div class="how-actions">
              <button type="button" class="btn btn-primary" onclick={() => store.dismissHow(true)}>{S.home.gotIt}</button>
              {#if !copy}
                <button type="button" class="btn" onclick={saveCopiesInstead}>{S.home.saveCopiesInstead}</button>
              {/if}
              <a class="btn-link" href={href('/backups')}>{S.home.whereBackups}</a>
            </div>
          </section>
        {/if}

        <section class="card pad" aria-labelledby="batch-h">
          {#if store.items.length > 0}
            <h2 id="batch-h">{S.home.resumeTitle}</h2>
            <div class="resume">
              <span class="folder-ico"><Icon name="folder" size={20} /></span>
              <div class="grow">
                <div class="label">{S.home.resumeNote(store.items.length, store.counts.needs)}</div>
              </div>
              <button type="button" class="btn" onclick={() => navigate('/grid')}>{S.home.resume}</button>
            </div>
          {:else}
            <h2 id="batch-h" class="sr-only">{S.home.resumeTitle}</h2>
          {/if}
          <div class="samples" class:first={store.items.length === 0}>
            <div class="caption">{S.home.samplesNote}</div>
            <button type="button" class="btn" disabled={store.opening} onclick={store.addSamples}>{S.home.trySamples}</button>
          </div>
        </section>
      </div>
    </div>
  </div>

  <footer class="foot">
    <Icon name="lock" size={16} />
    {S.home.offline}
  </footer>
</div>

<style>
  .top {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 20px 40px 8px;
  }

  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .actions {
    display: flex;
    gap: 8px;
  }

  .cols {
    display: grid;
    grid-template-columns: minmax(0, 7fr) minmax(0, 5fr);
    gap: 32px;
    padding: 8px 40px 24px;
    align-items: start;
  }

  .left,
  .right {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .drop {
    min-height: 236px;
    border: 2px dashed var(--switch-off);
    border-radius: 16px;
    background: var(--surface);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 24px 16px;
    text-align: center;
  }

  .drop.dragging {
    border-color: var(--accent);
    background: var(--accent-tint);
  }

  .drop-icon {
    width: 56px;
    height: 56px;
    border-radius: 50%;
    background: var(--accent-tint);
    color: var(--accent-text);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .drop-title {
    margin-top: 6px;
    font-size: 20px;
    font-weight: 600;
  }

  .drop-sub,
  .formats-note {
    color: var(--text-3);
  }

  .formats {
    margin-top: 4px;
    font-size: 12px;
    color: var(--text-3);
  }

  .formats-note {
    font-size: 12px;
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

  .pad {
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .shield {
    color: var(--good-fg);
    display: inline-flex;
  }

  .push {
    margin-left: auto;
  }

  .saving-line {
    color: var(--text);
  }

  .copy-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding-top: 12px;
    border-top: 1px solid var(--line-soft);
  }

  .label {
    font-weight: 500;
  }

  .caption {
    margin-top: 2px;
    font-size: 13px;
    color: var(--text-3);
    line-height: 1.4;
  }

  .info-card p {
    margin: 0;
    line-height: 1.5;
  }

  .how-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }

  .resume {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .folder-ico {
    width: 40px;
    height: 40px;
    border-radius: 8px;
    background: var(--bg);
    color: var(--text-2);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .samples {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--line-soft);
  }

  .samples.first {
    padding-top: 0;
    border-top: 0;
  }

  .samples .caption {
    margin: 0;
  }

  .foot {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 14px 40px 18px;
    color: var(--text-3);
    font-size: 13px;
  }

  @media (max-width: 900px) {
    .cols {
      grid-template-columns: minmax(0, 1fr);
      padding: 8px 20px 20px;
      gap: 16px;
    }

    .top {
      padding: 16px 20px 8px;
    }

    .foot {
      padding: 12px 20px 16px;
    }
  }
</style>
