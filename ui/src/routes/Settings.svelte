<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import Icon from '../lib/components/Icon.svelte';
  import Switch from '../lib/components/Switch.svelte';
  import { href, router } from '../lib/router.svelte.ts';
  import { routePath } from '../lib/routes.ts';
  import { store, type ThemeChoice } from '../lib/store.svelte.ts';
  import { RETENTION_OPTIONS, S } from '../lib/strings.ts';

  const back = $derived(
    router.previous && router.previous.name !== 'settings' ? routePath(router.previous) : store.items.length > 0 ? '/grid' : '/',
  );

  async function setRetention(value: string): Promise<void> {
    await store.updateSettings({ retentionDays: value === 'null' ? null : Number(value) });
  }

  async function setCopy(next: boolean): Promise<void> {
    const ok = await store.updateSettings({ saveAsCopy: next });
    if (ok) store.announce(next ? S.toasts.copyModeOn : S.toasts.copyModeOff);
  }

  const platformName = $derived(
    store.launch?.platform === 'windows' ? 'Windows' : store.launch?.platform === 'macos' ? 'macOS' : store.launch?.platform === 'linux' ? 'Linux' : '',
  );
</script>

<div class="page">
  <header class="top">
    <a class="icon-btn" href={href(back)} aria-label={S.nav.close}><Icon name="back" size={20} /></a>
    <h1 data-route-heading tabindex="-1">{S.settings.title}</h1>
  </header>

  <div class="page-scroll">
    <main class="col">
      <section class="card" aria-labelledby="s-saving">
        <h2 id="s-saving">{S.settings.saving}</h2>
        <div class="row">
          <div>
            <div class="label" id="copy-label">{S.settings.saveAsCopy}</div>
            <div class="help">{S.settings.saveAsCopyHelp}</div>
          </div>
          <Switch checked={store.settings.saveAsCopy} label={S.settings.saveAsCopy} onchange={setCopy} />
        </div>
        <div class="row last">
          <div>
            <label class="label" for="retention">{S.settings.backups}</label>
            <div class="help">{S.settings.backupsHelp}</div>
          </div>
          <div class="inline">
            <select id="retention" class="select" value={String(store.settings.retentionDays)} onchange={(e) => setRetention(e.currentTarget.value)}>
              {#each RETENTION_OPTIONS as o (String(o.value))}
                <option value={String(o.value)}>{o.label}</option>
              {/each}
            </select>
            <a class="btn" href={href('/backups')}>{S.settings.openBackups}</a>
          </div>
        </div>
      </section>

      <section class="card" aria-labelledby="s-look">
        <h2 id="s-look">{S.settings.appearance}</h2>
        <div class="row last">
          <label class="label" for="theme">{S.settings.theme}</label>
          <select id="theme" class="select" value={store.theme} onchange={(e) => store.setTheme(e.currentTarget.value as ThemeChoice)}>
            <option value="system">{S.settings.themeSystem}</option>
            <option value="light">{S.settings.themeLight}</option>
            <option value="dark">{S.settings.themeDark}</option>
          </select>
        </div>
      </section>

      <section class="card" aria-labelledby="s-priv">
        <h2 id="s-priv">{S.settings.privacy}</h2>
        <div class="privacy"><Icon name="lock" size={18} /> {S.settings.privacyNote}</div>
      </section>

      <section class="card" aria-labelledby="s-about">
        <h2 id="s-about">{S.settings.about}</h2>
        <dl class="about">
          <div><dt>{S.appName}</dt><dd class="mono">{S.settings.version} {store.launch?.version ?? ''}</dd></div>
          {#if platformName}<div><dt>{S.settings.platform}</dt><dd>{platformName}</dd></div>{/if}
        </dl>
        <div class="good-know">
          <div class="gk-title">{S.settings.notesTitle}</div>
          <ul>
            {#each S.settings.notes as n (n)}
              <li>{n}</li>
            {/each}
          </ul>
        </div>
      </section>
    </main>
  </div>
</div>

<style>
  .top {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 18px 24px 8px;
  }

  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .col {
    max-width: 760px;
    margin: 0 auto;
    padding: 8px 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .card {
    padding: 6px 20px 8px;
  }

  h2 {
    margin: 12px 0 4px;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 0.06em;
    color: var(--text-2);
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 12px 20px;
    padding: 12px 0;
    border-bottom: 1px solid var(--line-soft);
  }

  .row.last {
    border-bottom: 0;
  }

  .label {
    font-weight: 500;
  }

  .help {
    margin-top: 2px;
    font-size: 13px;
    color: var(--text-3);
  }

  .inline {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }

  .privacy {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 0 12px;
    color: var(--good-fg);
    font-weight: 500;
  }

  .about {
    margin: 4px 0 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 14px;
  }

  .about div {
    display: flex;
    justify-content: space-between;
    gap: 16px;
  }

  .about dt {
    font-weight: 500;
  }

  .about dd {
    margin: 0;
    color: var(--text-2);
  }

  .good-know {
    margin: 12px 0 10px;
    padding: 12px 14px;
    background: var(--check-bg);
    border: 1px solid var(--check-line);
    border-radius: 10px;
    color: var(--check-text-strong);
  }

  .gk-title {
    font-weight: 600;
    margin-bottom: 4px;
  }

  .good-know ul {
    margin: 0;
    padding-left: 18px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
    line-height: 1.45;
  }

  @media (max-width: 900px) {
    .top {
      padding-left: 14px;
    }

    .col {
      padding: 8px 14px 24px;
    }
  }
</style>
