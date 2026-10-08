/* The public gateway authenticates requests with an HttpOnly cookie. */
(() => {
  if (!window.APP_CONFIG?.loginSession) return;
  const originalFetch = window.fetch.bind(window);
  window.fetch = async (...args) => {
    const response = await originalFetch(...args);
    const input = args[0] instanceof Request ? args[0].url : args[0];
    const url = new URL(input, window.location.href);
    if (response.status === 401 && url.origin === location.origin && !url.pathname.startsWith('/auth/')) {
      location.replace('/login');
    }
    return response;
  };

  const node = (tag, className, text) => {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined) element.textContent = text;
    return element;
  };

  // Rough, display-only label. The user agent is reported by the browser and can be faked.
  const describeAgent = (agent) => {
    const browser = /Edg\//.test(agent) ? 'Edge' : /OPR\//.test(agent) ? 'Opera'
      : /Firefox\//.test(agent) ? 'Firefox' : /Chrome\//.test(agent) ? 'Chrome'
      : /Safari\//.test(agent) ? 'Safari' : 'Unknown browser';
    const system = /Android/.test(agent) ? 'Android' : /iPhone|iPad/.test(agent) ? 'iOS'
      : /Windows/.test(agent) ? 'Windows' : /Mac OS X/.test(agent) ? 'macOS'
      : /Linux/.test(agent) ? 'Linux' : 'unknown system';
    return `${browser} on ${system}`;
  };
  const when = (seconds) => new Date(seconds * 1000).toLocaleString();

  async function revoke(id) {
    const response = await originalFetch('/auth/sessions/revoke', {
      method: 'POST', credentials: 'same-origin',
      headers: {'Content-Type': 'application/json'}, body: JSON.stringify({id}),
    });
    if (response.status === 401) { location.replace('/login'); return false; }
    if (!response.ok && response.status !== 404) throw new Error('Could not revoke the session.');
    return true;
  }

  const overlay = node('div', 'confirm-overlay hidden');
  overlay.id = 'sessions-overlay';
  const dialog = node('div', 'confirm-dialog ota-dialog');
  dialog.setAttribute('role', 'dialog');
  dialog.setAttribute('aria-modal', 'true');
  dialog.setAttribute('aria-labelledby', 'sessions-title');
  const titleRow = node('div', 'dialog-title-row');
  const title = node('h2', '', 'Active sessions');
  title.id = 'sessions-title';
  const close = node('button', 'btn-ghost-sm', '✕');
  close.type = 'button';
  close.setAttribute('aria-label', 'Close active sessions');
  titleRow.append(title, close);
  const help = node('p', 'field-help',
    'Every browser currently logged in to this dashboard. Revoke any session you do not recognise, then reset your password and authenticator.');
  const list = node('ul', 'session-list');
  const status = node('p', 'field-help');
  status.setAttribute('role', 'status');
  const actions = node('div', 'confirm-actions');
  const revokeOthers = node('button', 'btn-danger-sm', 'Log out all other sessions');
  revokeOthers.type = 'button';
  actions.append(revokeOthers);
  dialog.append(titleRow, help, list, status, actions);
  overlay.append(dialog);
  document.body.append(overlay);

  async function render() {
    status.textContent = 'Loading…';
    list.replaceChildren();
    const response = await originalFetch('/auth/sessions', {credentials: 'same-origin'});
    if (response.status === 401) { location.replace('/login'); return; }
    if (!response.ok) { status.textContent = 'Could not load sessions.'; return; }
    const {sessions} = await response.json();
    status.textContent = '';
    revokeOthers.disabled = sessions.length < 2;
    for (const session of sessions) {
      const item = node('li', 'session-item' + (session.current ? ' session-current' : ''));
      const info = node('div', 'session-info');
      const name = node('strong', '', describeAgent(session.user_agent));
      if (session.current) name.append(node('span', 'session-badge', 'This device'));
      info.append(name,
        node('small', '', `Logged in ${when(session.created)} · Last active ${when(session.last_seen)}`),
        node('small', 'session-agent', session.user_agent || 'No user agent reported'));
      const button = node('button', session.current ? 'btn-sm' : 'btn-danger-sm',
        session.current ? 'Log out' : 'Revoke');
      button.type = 'button';
      button.addEventListener('click', async () => {
        button.disabled = true;
        try {
          if (!await revoke(session.id)) return;
          if (session.current) { location.replace('/login'); return; }
          await render();
        } catch (error) {
          status.textContent = error.message;
          button.disabled = false;
        }
      });
      item.append(info, button);
      list.append(item);
    }
  }

  const hide = () => overlay.classList.add('hidden');
  close.addEventListener('click', hide);
  overlay.addEventListener('click', (event) => { if (event.target === overlay) hide(); });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && !overlay.classList.contains('hidden')) hide();
  });
  revokeOthers.addEventListener('click', async () => {
    revokeOthers.disabled = true;
    try { if (await revoke('others')) await render(); }
    catch (error) { status.textContent = error.message; revokeOthers.disabled = false; }
  });

  const sessionsButton = node('button', 'btn-sm', 'Sessions');
  sessionsButton.type = 'button';
  sessionsButton.style.marginLeft = 'auto';
  sessionsButton.addEventListener('click', () => {
    overlay.classList.remove('hidden');
    close.focus();
    render().catch(() => { status.textContent = 'Could not load sessions.'; });
  });

  const logout = node('button', 'btn-sm', 'Log out');
  logout.type = 'button';
  logout.addEventListener('click', async () => {
    logout.disabled = true;
    try {
      const response = await originalFetch('/auth/logout', {method: 'POST', credentials: 'same-origin'});
      if (!response.ok) throw new Error('Logout failed');
      location.replace('/login');
    } catch {
      logout.disabled = false;
      logout.textContent = 'Retry log out';
    }
  });
  (document.querySelector('.topbar-actions') || document.querySelector('header') || document.body)
    .append(sessionsButton, logout);
})();
