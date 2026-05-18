export function setupEventFromForm(form) {
  const mode = form.mode;
  const event = {
    type: mode,
    repo: form.repo.trim()
  };

  if (!event.repo) {
    throw new Error('Repo path is required.');
  }

  if ((mode === 'resume' || mode === 'fork') && !form.sessionId.trim()) {
    throw new Error(`Session ID is required for ${mode}.`);
  }

  if (mode === 'resume' || mode === 'fork') {
    event.session = form.sessionId.trim();
  }

  if (mode === 'start' && form.temp) {
    event.temp = true;
  }

  if (form.profile.trim()) {
    event.profile = form.profile.trim();
  }

  if (Number.isInteger(form.autocompact)) {
    event.autocompact = form.autocompact;
  }

  return event;
}

export function displayStatus(state) {
  if (state === 'idle') return 'Idle';
  if (state === 'reasoning' || state === 'replying' || state === 'working') return 'Working';
  return 'Working';
}

export function compactText(value, fallback = '(empty)') {
  if (!value) return fallback;
  return value.length > 140 ? `${value.slice(0, 140)}...` : value;
}
