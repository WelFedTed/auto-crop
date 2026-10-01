<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  let {
    checked,
    label,
    onchange,
    disabled = false,
  }: { checked: boolean; label: string; onchange: (next: boolean) => void; disabled?: boolean } = $props();
</script>

<button
  type="button"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  class="switch"
  class:on={checked}
  onclick={() => onchange(!checked)}
>
  <span class="knob"></span>
</button>

<style>
  .switch {
    position: relative;
    flex-shrink: 0;
    width: 44px;
    height: 26px;
    padding: 0;
    border: 0;
    border-radius: 13px;
    background: var(--switch-off);
    /* the visible control stays 26 px high; a transparent border grows the hit area to 44 px */
  }

  .switch::before {
    content: '';
    position: absolute;
    inset: -9px -4px;
  }

  .switch.on {
    background: var(--accent);
  }

  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: #fff;
    transition: transform 0.12s ease;
  }

  .on .knob {
    transform: translateX(18px);
  }

  .switch:disabled {
    opacity: 0.5;
  }
</style>
