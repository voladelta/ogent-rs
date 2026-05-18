<script module>
  import { init, renderToHtml } from 'md4x/wasm';

  let initPromise;

  function ensureMd4x() {
    initPromise ??= init();
    return initPromise;
  }
</script>

<script>
  import DOMPurify from 'dompurify';

  let { content = '' } = $props();
  let html = $state('');
  let error = $state('');

  $effect(() => {
    const source = content;
    let cancelled = false;

    ensureMd4x()
      .then(() => {
        if (cancelled) return;
        const rendered = renderToHtml(source, { heal: true });
        html = DOMPurify.sanitize(rendered);
        error = '';
      })
      .catch((reason) => {
        if (cancelled) return;
        html = '';
        error = reason instanceof Error ? reason.message : String(reason);
      });

    return () => {
      cancelled = true;
    };
  });
</script>

{#if error}
  <div class="markdown-fallback">{content}</div>
  <div class="markdown-error">Markdown render failed: {error}</div>
{:else if html}
  <div class="markdown-body">{@html html}</div>
{:else}
  <div class="markdown-fallback">{content}</div>
{/if}
