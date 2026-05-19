<script>
  import { compactText, displayStatus, setupEventFromForm } from './lib/protocol.js';
  import MarkdownView from './lib/MarkdownView.svelte';
  import Popover from './lib/Popover.svelte';
  import SetupDialog from './lib/SetupDialog.svelte';
  const prismPalette = [
    '#5F4690',
    '#1D6996',
    '#38A6A5',
    '#0F8554',
    '#73AF48',
    '#EDAD08',
    '#E17C05',
    '#CC503E',
    '#94346E',
    '#6F4070',
    '#994E95',
    '#666666'
  ];
  const storageKey = 'ogent.frontend-app.settings';

  let workspaces = $state([
    { id: 'workspace-main', name: 'Main', color: prismPalette[0] },
    { id: 'workspace-research', name: 'Research', color: prismPalette[2] },
    { id: 'workspace-ship', name: 'Ship', color: prismPalette[5] }
  ]);
  let activeWorkspaceId = $state('workspace-main');
  let panes = $state([]);
  let settingsOpen = $state(false);
  let workspacePopoverOpen = $state(false);
  let setupOpen = $state(false);
  let setupWorkspaceId = $state('workspace-main');
  let collapsedWorkspaceIds = $state(['workspace-research', 'workspace-ship']);
  let workspaceDraft = $state({ name: '', color: prismPalette[3] });
  let setup = $state(loadSettings());

  let activeWorkspace = $derived(workspaces.find((workspace) => workspace.id === activeWorkspaceId) ?? workspaces[0]);
  let setupWorkspace = $derived(workspaces.find((workspace) => workspace.id === setupWorkspaceId) ?? activeWorkspace);
  let activePanes = $derived(panes.filter((pane) => pane.workspaceId === activeWorkspaceId));

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

  function paneCount(workspaceId) {
    return panes.filter((pane) => pane.workspaceId === workspaceId).length;
  }

  function panesForWorkspace(workspaceId) {
    return panes.filter((pane) => pane.workspaceId === workspaceId);
  }

  function workspaceCollapsed(workspaceId) {
    return collapsedWorkspaceIds.includes(workspaceId);
  }

  function toggleWorkspace(workspaceId) {
    collapsedWorkspaceIds = workspaceCollapsed(workspaceId)
      ? collapsedWorkspaceIds.filter((id) => id !== workspaceId)
      : [...collapsedWorkspaceIds, workspaceId];
  }

  function openSetup(workspaceId) {
    activeWorkspaceId = workspaceId;
    setupWorkspaceId = workspaceId;
    setupOpen = true;
  }

  function createWorkspace(close) {
    const name = workspaceDraft.name.trim();
    if (!name) return;
    const workspace = {
      id: createId('workspace'),
      name,
      color: workspaceDraft.color
    };
    workspaces.push(workspace);
    activeWorkspaceId = workspace.id;
    setupWorkspaceId = workspace.id;
    workspaceDraft = {
      name: '',
      color: prismPalette[workspaces.length % prismPalette.length]
    };
    close();
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

  function cleanTitle(value) {
    return typeof value === 'string' ? value.trim() : '';
  }

  function fallbackPaneTitle(pane) {
    return pane.sessionId ? `Session ${pane.sessionId.slice(0, 8)}` : 'Session';
  }

  function sessionSubtitle(pane) {
    const session = pane.sessionId ? pane.sessionId.slice(0, 12) : 'pending';
    return `${pane.mode} · ${session}`;
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

    const workspaceId = setupWorkspace?.id ?? activeWorkspaceId;
    activeWorkspaceId = workspaceId;

    const pane = {
      id: createId('pane'),
      workspaceId,
      title: setup.mode === 'start' ? 'New session' : `${setup.mode} session`,
      subtitle: 'Director stream',
      sessionTitle: '',
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
    setupOpen = false;
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
      const previousTitle = pane.title;
      pane.sessionId = event.session_id || pane.sessionId;
      pane.mode = event.mode || pane.mode;
      pane.profile = event.profile || pane.profile;
      pane.sessionTitle = cleanTitle(event.title) || pane.sessionTitle;
      pane.title = pane.sessionTitle || fallbackPaneTitle(pane);
      pane.subtitle = sessionSubtitle(pane);
      pane.status = 'Idle';
      if (event.status === 'updated') {
        if (pane.title !== previousTitle) {
          appendActivity(pane, createActivity('system', 'server', 'Session title updated', pane.title, { collapsed: true }));
        }
      } else {
        appendActivity(pane, createActivity('system', 'server', 'Session bound', `mode: ${pane.mode}\nrepo: ${event.repo}\nsession: ${pane.sessionId}`, { collapsed: true }));
      }
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
</script>

<svelte:head>
  <title>ogent workbench</title>
</svelte:head>

<div class="app-shell">
  <aside class="sidebar" aria-label="Workspace navigation">
    <header class="sidebar-head">
      <h1>ogent workbench</h1>
      <Popover bind:open={settingsOpen} placement="bottom-end">
        {#snippet trigger({ open, toggle })}
          <button class="icon-button settings-button" type="button" onclick={toggle} aria-label="Settings" aria-expanded={open}>
            <i class="icon settings"></i>
          </button>
        {/snippet}

        {#snippet children({ close })}
          <aside class="settings-popover" aria-label="Settings">
            <div class="drawer-head">
              <div>
                <p class="eyebrow">Settings</p>
                <h2>Defaults</h2>
              </div>
              <button class="icon-button" type="button" onclick={close} aria-label="Close settings">
                <i class="icon x"></i>
              </button>
            </div>
            <p>Session setup defaults are stored locally in this browser.</p>
            <dl>
              <div><dt>Active workspace</dt><dd>{activeWorkspace?.name}</dd></div>
              <div><dt>Default profile</dt><dd>{setup.profile}</dd></div>
              <div><dt>WebSocket</dt><dd>{setup.wsUrl}</dd></div>
            </dl>
          </aside>
        {/snippet}
      </Popover>
    </header>

    <div class="sidebar-section-head">
      <span class="workspace-label">Workspaces</span>
      <Popover bind:open={workspacePopoverOpen}>
        {#snippet trigger({ open, toggle })}
          <button
            class="icon-button add-workspace"
            type="button"
            onclick={toggle}
            aria-label="Add workspace"
            aria-expanded={open}
          >
              <i class="icon plus"></i>
            </button>
        {/snippet}

        {#snippet children({ close })}
          <form class="workspace-popover" onsubmit={(event) => { event.preventDefault(); createWorkspace(close); }}>
            <div>
              <p class="eyebrow">New workspace</p>
              <h2>Create workspace</h2>
            </div>
            <label>
              Workspace name
              <input bind:value={workspaceDraft.name} placeholder="e.g. Launch work" />
            </label>
            <div class="palette-field">
              <span>Color</span>
              <div class="palette-grid" aria-label="Prism palette">
                {#each prismPalette as color}
                  <button
                    class:selected={workspaceDraft.color === color}
                    type="button"
                    class="palette-swatch"
                    style={`--swatch: ${color}`}
                    aria-label={`Use ${color}`}
                    onclick={() => (workspaceDraft.color = color)}
                  ></button>
                {/each}
              </div>
            </div>
            <div class="popover-actions">
              <button type="button" onclick={close}>Cancel</button>
              <button class="primary" type="submit" disabled={!workspaceDraft.name.trim()}>Create</button>
            </div>
          </form>
        {/snippet}
      </Popover>
    </div>

    <nav class="workspace-list" aria-label="Workspaces">
      {#each workspaces as workspace}
        <section
          class:active={workspace.id === activeWorkspaceId}
          class="workspace-block"
          style={`--workspace-color: ${workspace.color}`}
        >
          <div class="workspace-row">
            <button
              class="workspace-disclosure"
              type="button"
              onclick={() => toggleWorkspace(workspace.id)}
              aria-label={`${workspaceCollapsed(workspace.id) ? 'Expand' : 'Collapse'} ${workspace.name}`}
              aria-expanded={!workspaceCollapsed(workspace.id)}
            >
              <i class={`icon ${workspaceCollapsed(workspace.id) ? 'caret-right' : 'caret-down'}`}></i>
            </button>
            <button
              class="workspace-select"
              type="button"
              onclick={() => (activeWorkspaceId = workspace.id)}
            >
              <span class="workspace-dot" aria-hidden="true"></span>
              <span>{workspace.name}</span>
            </button>
            <span class="workspace-count">{paneCount(workspace.id)}</span>
            <button
              class="icon-button add-session"
              type="button"
              onclick={() => openSetup(workspace.id)}
              aria-label={`Add session to ${workspace.name}`}
            >
            <i class="icon plus"></i>
            </button>
          </div>
          {#if !workspaceCollapsed(workspace.id)}
            <div class="session-list">
              {#each panesForWorkspace(workspace.id) as pane (pane.id)}
                <button
                  class:active={workspace.id === activeWorkspaceId}
                  class="session-nav-item"
                  type="button"
                  onclick={() => (activeWorkspaceId = workspace.id)}
                >
                  <span class="session-icon" aria-hidden="true">›_</span>
                  <span>{pane.title}</span>
                  <span class="session-state session-state-{statusTone(pane.status)}" aria-label={pane.status}></span>
                </button>
              {:else}
                <button class="empty-session" type="button" onclick={() => openSetup(workspace.id)}>
                  Start a session
                </button>
              {/each}
            </div>
          {/if}
        </section>
      {/each}
    </nav>
  </aside>

  <main class:has-sessions={activePanes.length > 0} class="pane-rail" aria-label={`${activeWorkspace?.name || 'Active'} sessions`}>
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
                {#if item.label === 'Assistant'}
                  <MarkdownView content={item.body} />
                {:else}
                  <div class="activity-body">{item.body}</div>
                {/if}
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
    {:else}
      <div class="empty-rail">
        <p>No sessions in {activeWorkspace?.name}.</p>
        <button class="primary" type="button" onclick={() => openSetup(activeWorkspace?.id)}>
          <i class="icon plus"></i>
          Add session
        </button>
      </div>
    {/each}
  </main>

  <SetupDialog bind:open={setupOpen} workspace={setupWorkspace} bind:setup onsubmit={startSession} />
</div>
