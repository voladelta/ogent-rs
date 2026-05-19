<script>
  const profileOptions = ['ds-flash', 'ds-flash-max', 'ds-pro', 'ds-pro-max', 'kimi', 'glm'];

  let {
    open = $bindable(false),
    group,
    setup = $bindable(),
    onsubmit
  } = $props();

  function close() {
    open = false;
  }

  $effect(() => {
    if (setup.mode !== 'start') setup.temp = false;
  });
</script>

{#if open}
  <div class="modal-layer" role="presentation">
    <button class="modal-scrim" type="button" onclick={close} aria-label="Close new session"></button>
    <div class="setup-dialog" role="dialog" aria-modal="true" aria-labelledby="new-session-title">
      <div class="setup-head">
        <div>
          <p class="eyebrow">Setup</p>
          <h2 id="new-session-title">New session</h2>
          <p>Connect a new Director stream to {group?.name}.</p>
        </div>
        <button class="icon-button" type="button" onclick={close} aria-label="Close new session">×</button>
      </div>

      <form class="setup-form" onsubmit={(event) => { event.preventDefault(); onsubmit?.(); }}>
        <label>
          WebSocket URL
          <input bind:value={setup.wsUrl} autocomplete="off" spellcheck="false" />
        </label>
        <label>
          Repo path
          <input bind:value={setup.repo} placeholder="/Users/mbp/Codehub/ogent-rs" autocomplete="off" spellcheck="false" />
        </label>
        <div class="form-grid">
          <label>
            Session mode
            <select bind:value={setup.mode}>
              <option value="start">start</option>
              <option value="resume">resume</option>
              <option value="fork">fork</option>
            </select>
          </label>
          <label>
            Profile
            <select bind:value={setup.profile}>
              {#each profileOptions as profile}
                <option value={profile}>{profile}</option>
              {/each}
            </select>
          </label>
        </div>
        {#if setup.mode !== 'start'}
          <label>
            Session ID
            <input bind:value={setup.sessionId} autocomplete="off" spellcheck="false" />
          </label>
        {/if}
        <div class="form-grid">
          <label>
            Autocompact
            <span class="percent-input">
              <input type="number" min="-1" max="100" bind:value={setup.autocompact} aria-label="Autocompact percent" />
              <span aria-hidden="true">%</span>
            </span>
          </label>
          <fieldset class="storage-field" disabled={setup.mode !== 'start'}>
            <legend>Session storage</legend>
            <div class="storage-options">
              <label class:active={!setup.temp} class="storage-option">
                <input type="radio" name="storage" checked={!setup.temp} onchange={() => (setup.temp = false)} />
                <span>Saved</span>
              </label>
              <label class:active={setup.temp} class="storage-option">
                <input type="radio" name="storage" checked={setup.temp} onchange={() => (setup.temp = true)} />
                <span>Temporary</span>
              </label>
            </div>
          </fieldset>
        </div>
        <button class="primary connect-button" type="submit">Connect</button>
      </form>
    </div>
  </div>
{/if}
