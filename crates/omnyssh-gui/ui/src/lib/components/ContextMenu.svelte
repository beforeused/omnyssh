<script lang="ts" module>
  import type { IconName } from '$lib/theme';

  export interface MenuItem {
    label: string;
    icon?: IconName;
    danger?: boolean;
    disabled?: boolean;
    run: () => void;
  }
</script>

<script lang="ts">
  // A right-click menu at the pointer. Closes on any outside press, Escape, scroll
  // or window blur; kept inside the viewport. Arrow keys move, Enter picks.
  import { onMount, tick } from 'svelte';
  import { Icon } from '$lib/theme';

  let {
    x,
    y,
    items,
    onClose
  }: { x: number; y: number; items: MenuItem[]; onClose: () => void } = $props();

  let menu = $state<HTMLDivElement | undefined>(undefined);
  let left = $state(0);
  let top = $state(0);

  onMount(() => {
    left = x;
    top = y;
    void tick().then(() => {
      if (!menu) return;
      const r = menu.getBoundingClientRect();
      left = Math.max(4, Math.min(x, window.innerWidth - r.width - 4));
      top = Math.max(4, Math.min(y, window.innerHeight - r.height - 4));
      menu.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
    });
  });

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const buttons = [...(menu?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = e.key === 'ArrowDown' ? at + 1 : at - 1;
    buttons[(next + buttons.length) % buttons.length]?.focus();
  }

  function onPointerDown(e: PointerEvent): void {
    if (menu && !menu.contains(e.target as Node)) onClose();
  }
</script>

<svelte:window
  onkeydown={onKeydown}
  onpointerdowncapture={onPointerDown}
  onblur={onClose}
  onresize={onClose}
  onwheel={onClose}
/>

<div
  bind:this={menu}
  role="menu"
  class="fixed z-50 min-w-44 rounded-xl border border-default bg-surface-raised p-1 shadow-soft"
  style="left: {left}px; top: {top}px"
>
  {#each items as item (item.label)}
    <button
      type="button"
      role="menuitem"
      disabled={item.disabled}
      class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-sm transition
        focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40
        {item.danger
        ? 'text-status-crit hover:bg-surface-inset focus-visible:bg-surface-inset'
        : 'text-fg hover:bg-surface-inset focus-visible:bg-surface-inset'}"
      onclick={() => {
        onClose();
        item.run();
      }}
    >
      {#if item.icon}<Icon name={item.icon} size={14} />{/if}
      {item.label}
    </button>
  {/each}
</div>
