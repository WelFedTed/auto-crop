<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import type { Tier } from '../review.ts';
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';

  let { tier, label }: { tier: Tier; label?: string } = $props();
</script>

<!-- Icon and word, never colour alone (PLAN 6.7). -->
<span class="badge {tier === 'analysing' ? 'neutral' : tier}">
  <Icon name={tier === 'analysing' ? 'analysing' : tier} size={14} stroke={2} />
  {label ?? S.tier[tier]}
</span>
