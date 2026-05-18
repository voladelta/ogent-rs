<script>
  import { compactText, displayStatus, setupEventFromForm } from './lib/protocol.js';

  const profileOptions = ['ds-flash', 'ds-flash-max', 'ds-pro', 'ds-pro-max', 'kimi', 'glm'];
  const storageKey = 'ogent.frontend-app.settings';

  let groups = $state([
    { id: 'group-main', name: 'Main', color: 'teal' },
    { id: 'group-research', name: 'Research', color: 'amber' },
    { id: 'group-ship', name: 'Ship', color: 'green' }
  ]);
  let activeGroupId = $state('group-main');
  let panes = $state([]);
  let settingsOpen = $state(false);
  let setup = $state(loadSettings());

  let activeGroup = $derived(groups.find((group) => group.id === activeGroupId) ?? groups[0]);
  let activePanes = $derived(panes.filter((pane) => pane.groupId === activeGroupId));

  function defaultSettings() {
    return {
      wsUrl: 'ws://127.0.0.1:9876',
      repo: '',
      mode: 'start',
      sessionId: '',
      profile: 'ds-flash',
      autocompact: 80,
      temp: false
    };
  }

  function loadSettings() {
    try {
      const raw = localStorage.getItem(storageKey);
      return raw ? { ...defaultSettings(), ...JSON.parse(raw) } : defaultSettings();
    } catch {
      return defaultSettings();
    }
  }

  function saveSettings() {
    localStorage.setItem(storageKey, JSON.stringify(setup));
  }

  function createId(prefix) {
    return `${prefix}-${Math.random().toString(36).slice(2, 9)}`;
  }

  function addGroup() {
    const name = window.prompt('Group name');
    if (!name?.trim()) return;
    const colors = ['teal', 'amber', 'green', 'rose', 'blue'];
    const group = {
      id: createId('group'),
      name: name.trim(),
      color: colors[groups.length % colors.length]
    };
    groups.push(group);
    activeGroupId = group.id;
  }

  function createActivity(kind, source, label, body, options = {}) {
    return {
      id: createId('event'),
      kind,
      source,
      label,
      body,
      collapsed: options.collapsed ?? false,
      tone: options.tone ?? 'normal',
      createdAt: new Date()
    };
  }

  function appendActivity(pane, activity) {
    pane.activity.push(activity);
    queueMicrotask(() => {
      const el = document.querySelector(`[data-pane-log="${pane.id}"]`);
      if (el) el.scrollTop = el.scrollHeight;
    });
  }

  function startSession() {
    let event;
    try {
      event = setupEventFromForm(setup);
    } catch (error) {
      window.alert(error.message);
      return;
    }

    saveSettings();

    const pane = {
      id: createId('pane'),
      groupId: activeGroupId,
      title: setup.mode === 'start' ? 'New session' : `${setup.mode} session`,
      subtitle: 'Director stream',
      wsUrl: setup.wsUrl.trim(),
      sessionId: setup.mode === 'start' ? '' : setup.sessionId.trim(),
      mode: setup.mode,
      profile: setup.profile.trim() || 'ds-flash',
      model: 'pending',
      tokens: 0,
      agentState: 'idle',
      status: 'Working',
      connection: 'connecting',
      draft: '',
      activity: [
        createActivity('system', 'system', 'Setup', JSON.stringify(event, null, 2), { collapsed: true })
      ],
      socket: null
    };

    panes.push(pane);
    connectPane(panes[panes.length - 1], event);
  }

  function connectPane(pane, setupEvent) {
    let socket;
    try {
      socket = new WebSocket(pane.wsUrl);
    } catch (error) {
      pane.connection = 'error';
      pane.status = 'Failed';
      appendActivity(pane, createActivity('error', 'system', 'Socket error', error.message, { tone: 'error' }));
      return;
    }

    pane.socket = socket;

    socket.addEventListener('open', () => {
      pane.connection = 'open';
      socket.send(JSON.stringify(setupEvent));
    });

    socket.addEventListener('message', (message) => {
      handleServerEvent(pane, message.data);
    });

    socket.addEventListener('error', () => {
      pane.connection = 'error';
      pane.status = 'Failed';
      appendActivity(pane, createActivity('error', 'server', 'WebSocket error', 'Check that ogent --serve is running and reachable.', { tone: 'error' }));
    });

    socket.addEventListener('close', () => {
      pane.connection = 'closed';
      if (pane.status !== 'Failed') pane.status = 'Idle';
      appendActivity(pane, createActivity('system', 'server', 'Disconnected', 'Socket closed.'));
    });
  }

  function handleServerEvent(pane, raw) {
    let event;
    try {
      event = JSON.parse(raw);
    } catch {
      appendActivity(pane, createActivity('error', 'server', 'Invalid JSON', raw, { tone: 'error' }));
      return;
    }

    if (event.type === 'session') {
      pane.sessionId = event.session_id || pane.sessionId;
      pane.mode = event.mode || pane.mode;
      pane.profile = event.profile || pane.profile;
      pane.title = pane.sessionId ? `Session ${pane.sessionId.slice(0, 8)}` : 'Session';
      pane.status = 'Idle';
      appendActivity(pane, createActivity('system', 'server', 'Session bound', `mode: ${pane.mode}\nrepo: ${event.repo}\nsession: ${pane.sessionId}`, { collapsed: true }));
      return;
    }

    if (event.type === 'status') {
      pane.agentState = event.state || pane.agentState;
      pane.tokens = event.tokens ?? pane.tokens;
      pane.profile = event.profile || pane.profile;
      pane.model = event.model || pane.model;
      pane.status = displayStatus(event.state);
      return;
    }

    if (event.type === 'error') {
      pane.status = 'Failed';
      appendActivity(pane, createActivity('error', 'server', event.code || 'Error', event.message || 'Unknown error', { tone: 'error' }));
      return;
    }

    if (event.type === 'message') {
      appendTranscriptMessage(pane, event);
    }
  }

  function appendTranscriptMessage(pane, event) {
    if (event.reasoning_content) {
      appendActivity(pane, createActivity('reasoning', event.source || 'director', 'Reasoning · collapsed', event.reasoning_content, { collapsed: true }));
    }

    if (Array.isArray(event.tool_calls) && event.tool_calls.length > 0) {
      const label = event.tool_calls.length === 1
        ? `Tool call · ${event.tool_calls[0]?.function?.name || 'collapsed'}`
        : `Tool calls · ${event.tool_calls.length} collapsed`;
      appendActivity(pane, createActivity('tool', event.source || 'director', label, JSON.stringify(event.tool_calls, null, 2), { collapsed: true }));
    }

    if (event.role === 'tool') {
      appendActivity(pane, createActivity('tool', event.source || 'tool', 'Tool result · collapsed', event.content || '', { collapsed: true }));
      return;
    }

    if (event.content) {
      const source = event.source || 'director';
      const label = event.role === 'assistant' ? 'Assistant' : event.role || 'Message';
      appendActivity(pane, createActivity('content', source, label, event.content));
    }
  }

  function sendPrompt(pane) {
    const content = pane.draft.trim();
    if (!content || pane.socket?.readyState !== WebSocket.OPEN) return;
    pane.socket.send(JSON.stringify({ type: 'message', content }));
    pane.draft = '';
    pane.status = 'Working';
    appendActivity(pane, createActivity('user', 'you', 'Prompt', content));
  }

  function sendControl(pane, type) {
    if (pane.socket?.readyState !== WebSocket.OPEN) return;
    pane.socket.send(JSON.stringify({ type }));
    appendActivity(pane, createActivity('system', 'you', type, `Sent ${type}.`, { collapsed: true }));
  }

  function removePane(pane) {
    if (pane.socket?.readyState === WebSocket.OPEN) {
      pane.socket.send(JSON.stringify({ type: 'exit' }));
      pane.socket.close();
    }
    panes = panes.filter((candidate) => candidate.id !== pane.id);
  }

  function statusTone(status) {
    return status.toLowerCase().replace(/\s+/g, '-');
  }

  function activitySummary(item) {
    const chars = item.body?.length ?? 0;
    if (item.kind === 'reasoning') return `${chars.toLocaleString()} chars`;
    if (item.kind === 'tool') return compactText(item.body, 'collapsed');
    return '';
  }

  function toggleActivity(item) {
    item.collapsed = !item.collapsed;
  }

  $effect(() => {
    if (setup.mode !== 'start') setup.temp = false;
  });
</script>

<svelte:head>
  <title>ogent workbench</title>
</svelte:head>

<div class="app-shell">
  <header class="group-bar">
    <div class="group-label">Groups</div>
    <nav class="groups" aria-label="Groups">
      {#each groups as group}
        <button
          class:active={group.id === activeGroupId}
          class="group-tab color-{group.color}"
          type="button"
          onclick={() => (activeGroupId = group.id)}
        >
          {group.name}
        </button>
      {/each}
      <button class="group-tab add-group" type="button" onclick={addGroup} aria-label="Add group">+</button>
    </nav>
    <button class="settings-button" type="button" onclick={() => (settingsOpen = !settingsOpen)}>Settings</button>
  </header>

  <main class="pane-rail" aria-label={`${activeGroup?.name || 'Active'} sessions`}>
    {#each activePanes as pane (pane.id)}
      <section class="session-pane" aria-label={pane.title}>
        <div class="pane-head">
          <div>
            <p class="eyebrow">Transcript</p>
            <h2>{pane.title}</h2>
            <p class="subtitle">{pane.subtitle}</p>
          </div>
          <div class="pane-actions">
            <span class="status status-{statusTone(pane.status)}">{pane.status}</span>
            <button type="button" onclick={() => sendControl(pane, 'cancel')}>Cancel</button>
            <button type="button" onclick={() => removePane(pane)} aria-label="Close pane">Close</button>
          </div>
        </div>

        <div class="transcript" data-pane-log={pane.id}>
          {#each pane.activity as item (item.id)}
            <article class="activity-item item-{item.kind} tone-{item.tone}">
              {#if item.collapsed}
                <button class="collapse-row" type="button" onclick={() => toggleActivity(item)}>
                  <span>{item.label}</span>
                  <span>{activitySummary(item)}</span>
                </button>
              {:else if item.kind === 'reasoning' || item.kind === 'tool' || item.kind === 'system'}
                <button class="collapse-row open" type="button" onclick={() => toggleActivity(item)}>
                  <span>{item.label}</span>
                  <span>hide</span>
                </button>
                <pre>{item.body}</pre>
              {:else}
                <div class="activity-meta">
                  <span>{item.label}</span>
                  <span>{item.source}</span>
                </div>
                <div class="activity-body">{item.body}</div>
              {/if}
            </article>
          {/each}
        </div>

        <form class="prompt-box" onsubmit={(event) => { event.preventDefault(); sendPrompt(pane); }}>
          <label for={`prompt-${pane.id}`}>Prompt</label>
          <textarea id={`prompt-${pane.id}`} bind:value={pane.draft} placeholder="Tell this session what to do next..."></textarea>
          <div class="prompt-actions">
            <button type="button" onclick={() => sendControl(pane, 'compact')}>Compact</button>
            <button class="primary" type="submit" disabled={pane.socket?.readyState !== WebSocket.OPEN}>Send</button>
          </div>
        </form>

        <footer class="pane-foot">
          <span>profile <strong>{pane.profile}</strong></span>
          <span>tokens <strong>{pane.tokens.toLocaleString()}</strong></span>
          <span>{pane.connection}</span>
        </footer>
      </section>
    {/each}

    <section class="setup-pane" aria-label="Setup new session">
      <div class="setup-head">
        <p class="eyebrow">Setup</p>
        <h2>Add session</h2>
        <p>Connect a new Director stream to this group.</p>
      </div>

      <form class="setup-form" onsubmit={(event) => { event.preventDefault(); startSession(); }}>
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
            Mode
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
            <input type="number" min="-1" max="100" bind:value={setup.autocompact} />
          </label>
          <label class="check-row">
            <input type="checkbox" bind:checked={setup.temp} disabled={setup.mode !== 'start'} />
            <span>Temporary</span>
          </label>
        </div>
        <button class="primary connect-button" type="submit">Connect</button>
      </form>
    </section>
  </main>

  {#if settingsOpen}
    <aside class="settings-drawer" aria-label="Settings">
      <div class="drawer-head">
        <div>
          <p class="eyebrow">Settings</p>
          <h2>Defaults</h2>
        </div>
        <button type="button" onclick={() => (settingsOpen = false)}>Close</button>
      </div>
      <p>Settings are intentionally small for now. Session setup defaults are stored locally in this browser.</p>
      <dl>
        <div><dt>Active group</dt><dd>{activeGroup?.name}</dd></div>
        <div><dt>Default profile</dt><dd>{setup.profile}</dd></div>
        <div><dt>WebSocket</dt><dd>{setup.wsUrl}</dd></div>
      </dl>
    </aside>
  {/if}
</div>
