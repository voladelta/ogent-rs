(function () {
  "use strict";

  const storageKey = "ogent.frontend-basic.settings";
  const elements = {
    setupForm: document.querySelector("#setup-form"),
    wsUrl: document.querySelector("#ws-url"),
    repo: document.querySelector("#repo"),
    mode: document.querySelector("#mode"),
    profile: document.querySelector("#profile"),
    sessionField: document.querySelector("#session-field"),
    sessionId: document.querySelector("#session-id"),
    autocompact: document.querySelector("#autocompact"),
    temp: document.querySelector("#temp"),
    connectButton: document.querySelector("#connect-button"),
    disconnectButton: document.querySelector("#disconnect-button"),
    connectionPill: document.querySelector("#connection-pill"),
    connectionLabel: document.querySelector("#connection-label"),
    agentState: document.querySelector("#agent-state"),
    tokenCount: document.querySelector("#token-count"),
    currentTitle: document.querySelector("#current-title"),
    currentSession: document.querySelector("#current-session"),
    currentMode: document.querySelector("#current-mode"),
    currentProfile: document.querySelector("#current-profile"),
    currentModel: document.querySelector("#current-model"),
    eventLog: document.querySelector("#event-log"),
    messageForm: document.querySelector("#message-form"),
    messageInput: document.querySelector("#message-input"),
    sendButton: document.querySelector("#send-button"),
    cancelButton: document.querySelector("#cancel-button"),
    compactButton: document.querySelector("#compact-button"),
    newButton: document.querySelector("#new-button"),
    clearButton: document.querySelector("#clear-button"),
    compactFocus: document.querySelector("#compact-focus"),
  };

  let socket = null;
  let connected = false;
  let initialized = false;
  let connecting = false;

  function loadSettings() {
    try {
      const raw = window.localStorage.getItem(storageKey);
      if (!raw) return;
      const settings = JSON.parse(raw);
      if (settings.wsUrl) elements.wsUrl.value = settings.wsUrl;
      if (settings.repo) elements.repo.value = settings.repo;
      if (settings.mode) elements.mode.value = settings.mode;
      if (settings.profile) elements.profile.value = settings.profile;
      if (settings.sessionId) elements.sessionId.value = settings.sessionId;
      if (Number.isInteger(settings.autocompact))
        elements.autocompact.value = String(settings.autocompact);
      elements.temp.checked = Boolean(settings.temp);
    } catch (error) {
      appendEvent(
        "system",
        "settings",
        "Could not read saved settings: " + error.message,
        "event-error",
      );
    }
  }

  function saveSettings() {
    const settings = {
      wsUrl: elements.wsUrl.value.trim(),
      repo: elements.repo.value.trim(),
      mode: elements.mode.value,
      profile: elements.profile.value.trim(),
      sessionId: elements.sessionId.value.trim(),
      autocompact: parseAutocompact(),
      temp: elements.temp.checked,
    };
    try {
      window.localStorage.setItem(storageKey, JSON.stringify(settings));
    } catch (error) {
      appendEvent(
        "system",
        "settings",
        "Could not save settings: " + error.message,
        "event-error",
      );
    }
  }

  function parseAutocompact() {
    const raw = elements.autocompact.value.trim();
    if (raw === "") return undefined;
    const parsed = Number(raw);
    if (!Number.isInteger(parsed)) return undefined;
    return parsed;
  }

  function setConnectionState(state, label) {
    elements.connectionPill.dataset.state = state;
    elements.connectionLabel.textContent = label;
  }

  function setConnectedState(nextConnected) {
    connected = nextConnected;
    elements.connectButton.disabled =
      connecting || (nextConnected && initialized);
    elements.connectButton.textContent =
      nextConnected && !initialized ? "Send setup" : "Connect";
    elements.disconnectButton.disabled = !nextConnected;
    elements.sendButton.disabled = !nextConnected || !initialized;
    elements.messageInput.disabled = !nextConnected || !initialized;
    elements.cancelButton.disabled = !nextConnected || !initialized;
    elements.compactButton.disabled = !nextConnected || !initialized;
    elements.newButton.disabled = !nextConnected || !initialized;
    elements.compactFocus.disabled = !nextConnected || !initialized;
  }

  function setInitialized(nextInitialized) {
    initialized = nextInitialized;
    setConnectedState(connected);
  }

  function updateModeFields() {
    const needsSession =
      elements.mode.value === "resume" || elements.mode.value === "fork";
    elements.sessionField.classList.toggle("is-hidden", !needsSession);
    elements.sessionId.required = needsSession;
    elements.temp.disabled = elements.mode.value !== "start";
    if (elements.mode.value !== "start") elements.temp.checked = false;
  }

  function appendEvent(source, role, content, className) {
    const article = document.createElement("article");
    article.className = "event " + (className || eventClass(source, role));

    const meta = document.createElement("div");
    meta.className = "event-meta";
    meta.textContent = source + "\n" + role;

    const body = document.createElement("div");
    body.className = "event-body";
    body.textContent = content || "(empty)";

    article.append(meta, body);
    elements.eventLog.appendChild(article);
    elements.eventLog.scrollTop = elements.eventLog.scrollHeight;
  }

  function eventClass(source, role) {
    if (role === "tool") return "event-tool";
    if (source && source !== "director" && source !== "system")
      return "event-worker";
    if (source === "director") return "event-director";
    return "event-system";
  }

  function sendEvent(event) {
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      appendEvent("system", "error", "Socket is not open.", "event-error");
      return false;
    }
    socket.send(JSON.stringify(event));
    return true;
  }

  function buildSetupEvent() {
    const mode = elements.mode.value;
    const repo = elements.repo.value.trim();
    const profile = elements.profile.value.trim();
    const autocompact = parseAutocompact();
    const event = { type: mode, repo: repo };

    if (!repo) throw new Error("Repo path is required.");
    if (
      (mode === "resume" || mode === "fork") &&
      !elements.sessionId.value.trim()
    ) {
      throw new Error("Session ID is required for " + mode + ".");
    }
    if (mode === "resume" || mode === "fork") {
      event.session = elements.sessionId.value.trim();
    }
    if (mode === "start" && elements.temp.checked) {
      event.temp = true;
    }
    if (profile) event.profile = profile;
    if (autocompact !== undefined) event.autocompact = autocompact;
    return event;
  }

  function connect(event) {
    event.preventDefault();
    if (
      connecting ||
      (socket && socket.readyState === WebSocket.OPEN && initialized)
    )
      return;

    let setupEvent;
    try {
      setupEvent = buildSetupEvent();
      saveSettings();
    } catch (error) {
      appendEvent("system", "setup", error.message, "event-error");
      return;
    }

    if (socket && socket.readyState === WebSocket.OPEN && !initialized) {
      appendEvent("system", "setup", JSON.stringify(setupEvent));
      sendEvent(setupEvent);
      elements.connectButton.disabled = true;
      return;
    }

    setConnectionState("connecting", "Connecting");
    connecting = true;
    setConnectedState(false);
    appendEvent("system", "connect", "Opening " + elements.wsUrl.value.trim());

    try {
      socket = new WebSocket(elements.wsUrl.value.trim());
    } catch (error) {
      connecting = false;
      setConnectionState("error", "Invalid URL");
      setConnectedState(false);
      appendEvent("system", "connect", error.message, "event-error");
      return;
    }

    socket.addEventListener("open", function () {
      connecting = false;
      setConnectionState("open", "Connected");
      setConnectedState(true);
      appendEvent("system", "setup", JSON.stringify(setupEvent));
      sendEvent(setupEvent);
    });

    socket.addEventListener("message", function (messageEvent) {
      handleServerEvent(messageEvent.data);
    });

    socket.addEventListener("error", function () {
      connecting = false;
      setConnectionState("error", "Socket error");
      setConnectedState(false);
      appendEvent(
        "system",
        "error",
        "WebSocket error. Check that ogent --serve is running and the URL is reachable.",
        "event-error",
      );
    });

    socket.addEventListener("close", function () {
      connecting = false;
      setConnectionState("closed", "Disconnected");
      setConnectedState(false);
      setInitialized(false);
      appendEvent("system", "close", "Socket closed.");
    });
  }

  function disconnect() {
    if (!socket) return;
    if (socket.readyState === WebSocket.OPEN) {
      sendEvent({ type: "exit" });
    }
    socket.close();
  }

  function handleServerEvent(raw) {
    let event;
    try {
      event = JSON.parse(raw);
    } catch (error) {
      appendEvent("server", "invalid_json", raw, "event-error");
      console.error(error);
      return;
    }

    switch (event.type) {
      case "session":
        handleSession(event);
        break;
      case "status":
        handleStatus(event);
        break;
      case "message":
        handleMessage(event);
        break;
      case "error":
        appendEvent(
          "server",
          event.code || "error",
          event.message || "Unknown error",
          "event-error",
        );
        if (!initialized) {
          elements.connectButton.disabled = false;
          elements.connectButton.textContent = "Send setup";
        }
        break;
      default:
        appendEvent(
          "server",
          "unknown",
          JSON.stringify(event, null, 2),
          "event-error",
        );
    }
  }

  function handleSession(event) {
    setInitialized(true);
    const previousTitle = elements.currentTitle.textContent;
    const title = typeof event.title === "string" ? event.title.trim() : "";
    if (event.status === "updated") {
      if (title) elements.currentTitle.textContent = title;
    } else {
      elements.currentTitle.textContent = title || "none";
    }
    elements.currentSession.textContent = event.session_id || "unknown";
    elements.currentMode.textContent = event.mode || "unknown";
    elements.currentProfile.textContent = event.profile || "unknown";
    if (event.session_id) elements.sessionId.value = event.session_id;
    if (event.status === "updated") {
      if (title && title !== previousTitle) {
        appendEvent("server", "session", "Title updated: " + title, "event-status");
      }
      return;
    }
    appendEvent(
      "server",
      "session",
      "Session " + event.session_id + " bound to " + event.repo,
      "event-status",
    );
  }

  function handleStatus(event) {
    elements.agentState.textContent = event.state || "unknown";
    elements.tokenCount.textContent = String(event.tokens || 0);
    elements.currentProfile.textContent = event.profile || "unknown";
    elements.currentModel.textContent = event.model || "unknown";
  }

  function handleMessage(event) {
    const contentParts = [];
    if (event.reasoning_content) {
      contentParts.push("[reasoning]\n" + event.reasoning_content);
    }
    if (event.content) {
      contentParts.push(event.content);
    }
    if (Array.isArray(event.tool_calls) && event.tool_calls.length > 0) {
      contentParts.push(
        "[tool calls]\n" + JSON.stringify(event.tool_calls, null, 2),
      );
    }
    appendEvent(
      event.source || "server",
      event.role || "message",
      contentParts.join("\n\n"),
    );
  }

  function sendMessage(event) {
    event.preventDefault();
    const content = elements.messageInput.value.trim();
    if (!content) return;
    if (sendEvent({ type: "message", content: content })) {
      appendEvent("you", "message", content);
      elements.messageInput.value = "";
      elements.messageInput.focus();
    }
  }

  function compact() {
    const focus = elements.compactFocus.value.trim();
    sendEvent(focus ? { type: "compact", focus: focus } : { type: "compact" });
  }

  function newSession() {
    const ok = window.confirm(
      "The current websocket protocol does not emit the replacement session ID after new. Continue?",
    );
    if (ok) sendEvent({ type: "new" });
  }

  function clearLog() {
    elements.eventLog.innerHTML = "";
    appendEvent("system", "clear", "Transcript cleared locally.");
  }

  elements.setupForm.addEventListener("submit", connect);
  elements.disconnectButton.addEventListener("click", disconnect);
  elements.mode.addEventListener("change", updateModeFields);
  elements.messageForm.addEventListener("submit", sendMessage);
  elements.cancelButton.addEventListener("click", function () {
    sendEvent({ type: "cancel" });
  });
  elements.compactButton.addEventListener("click", compact);
  elements.newButton.addEventListener("click", newSession);
  elements.clearButton.addEventListener("click", clearLog);
  elements.messageInput.addEventListener("keydown", function (event) {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      elements.messageForm.requestSubmit();
    }
  });

  loadSettings();
  updateModeFields();
  setConnectedState(false);
})();
