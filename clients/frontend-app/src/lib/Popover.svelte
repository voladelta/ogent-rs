<script>
  import { autoUpdate, computePosition, flip, offset, shift } from '@floating-ui/dom';

  let {
    open = $bindable(false),
    placement = 'bottom-start',
    trigger,
    children
  } = $props();

  let triggerEl = $state();
  let panelEl = $state();

  function close() {
    open = false;
  }

  function toggle() {
    open = !open;
  }

  async function updatePosition() {
    if (!triggerEl || !panelEl) return;
    const { x, y } = await computePosition(triggerEl, panelEl, {
      placement,
      middleware: [offset(8), flip(), shift({ padding: 12 })]
    });
    Object.assign(panelEl.style, {
      left: `${x}px`,
      top: `${y}px`
    });
  }

  $effect(() => {
    if (!open || !triggerEl || !panelEl) return;

    const cleanupPosition = autoUpdate(triggerEl, panelEl, updatePosition);

    function handlePointerDown(event) {
      if (triggerEl?.contains(event.target) || panelEl?.contains(event.target)) return;
      close();
    }

    function handleKeyDown(event) {
      if (event.key === 'Escape') close();
    }

    updatePosition();
    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);

    return () => {
      cleanupPosition();
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  });
</script>

<span class="popover-anchor" bind:this={triggerEl}>
  {@render trigger({ open, toggle, close })}
</span>

{#if open}
  <div class="popover-panel" bind:this={panelEl} role="dialog">
    {@render children({ close })}
  </div>
{/if}
