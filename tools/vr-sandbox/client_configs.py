"""The two agent-client files a test build's bridge writes into: `~/.claude.json` and `~/.codex/config.toml`.

A test program started in a sandbox session registers its own bridge there (crates/orbiters-desktop/src/bridge/clients.rs:
the `orbiters-vrchat` server, with the test build's port and token). Every other Claude Code and Codex session on this
machine keeps writing these files during the session (SBX-2), so they are never put back as bytes: the session records the
bridge's own entry as it was at the start and, at restore, puts back exactly that entry (or takes it away when there was
none), leaving everything else as the other programs left it.

- `.claude.json`: the user-scope `mcpServers.orbiters-vrchat` and the home folder's local-scope
  `projects.<home>.mcpServers.orbiters-vrchat` (`claude mcp add --scope user` / `remove --scope user|local`, run from home).
- `config.toml`: the `[mcp_servers.orbiters-vrchat]` table and its sub-tables, edited as clients.rs does (only its own lines;
  comments stay; a block added goes at the end).
"""
import json
import os
from pathlib import Path
import time

SERVER = 'orbiters-vrchat'


def write_atomic(path, data):
    """Temp file beside it, flushed to disk, then renamed over it."""
    path = Path(path)
    tmp = path.with_name(path.name + '.sandbox-tmp')
    with open(tmp, 'wb') as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    try:
        for attempt in range(20):
            try:
                os.replace(tmp, path)
                return
            except PermissionError as error:
                if getattr(error, 'winerror', None) not in (5, 32, 33) or attempt == 19:
                    raise
                time.sleep(.05)
    finally:
        tmp.unlink(missing_ok=True)


# ---- Claude Code ------------------------------------------------------------------------------------------------------
def _norm_dir(text):
    return os.path.normcase(os.path.normpath(str(text).replace('/', os.sep)))


def claude_snapshot(text, home):
    """What the bridge's entries are in a `.claude.json` text: {'user': value|None, 'local': {project key: value|None}}."""
    data = json.loads(text) if text.strip() else {}
    snapshot = {'user': (data.get('mcpServers') or {}).get(SERVER), 'local': {}}
    for key, project in (data.get('projects') or {}).items():
        if _norm_dir(key) == _norm_dir(home) and isinstance(project, dict):
            snapshot['local'][key] = (project.get('mcpServers') or {}).get(SERVER)
    return snapshot


def _set(servers_owner, value):
    """Sets or removes the bridge's entry in a dict holding `mcpServers`. True when it changed."""
    servers = servers_owner.get('mcpServers')
    current = servers.get(SERVER) if isinstance(servers, dict) else None
    if current == value:
        return False
    if value is None:
        del servers[SERVER]
    else:
        if not isinstance(servers, dict):
            servers = servers_owner['mcpServers'] = {}
        servers[SERVER] = value
    return True


def claude_merged(text, snapshot, home):
    """`text` with the bridge's entries as in `snapshot`, everything else as it is; None when nothing differs."""
    data = json.loads(text)
    changed = _set(data, snapshot['user'])
    for key, project in (data.get('projects') or {}).items():
        if _norm_dir(key) == _norm_dir(home) and isinstance(project, dict):
            changed |= _set(project, snapshot['local'].get(key))
    if not changed:
        return None
    newline = '\r\n' if '\r\n' in text else '\n'
    out = json.dumps(data, indent=2, ensure_ascii=False).replace('\n', newline)
    return out + (newline if text.endswith(('\n', '\r')) else '')


# ---- Codex ------------------------------------------------------------------------------------------------------------
def _ours(trimmed):
    return trimmed == f'[mcp_servers.{SERVER}]' or trimmed.startswith(f'[mcp_servers.{SERVER}.')


def codex_without(config):
    """`config` without the bridge's tables: a port of clients.rs `without_codex_entry` (comments stay)."""
    kept, ours, pending = [], False, ''
    for line in config.splitlines(keepends=True):
        trimmed = line.strip()
        if trimmed.startswith('['):
            ours = _ours(trimmed)
            if ours:
                pending = ''
                continue
        if ours and not trimmed.startswith('#'):
            continue
        if not trimmed:
            pending += line
            continue
        kept.append(pending)
        pending = ''
        kept.append(line)
    kept.append(pending)
    return ''.join(kept)


def codex_snapshot(config):
    """The bridge's own lines (headers and keys, no comments or blank lines), line ends stripped; None when absent."""
    lines, ours = [], False
    for line in config.splitlines():
        trimmed = line.strip()
        if trimmed.startswith('['):
            ours = _ours(trimmed)
        if ours and trimmed and not trimmed.startswith('#'):
            lines.append(line.rstrip())
    return lines or None


def codex_merged(config, snapshot):
    """`config` with the bridge's block as in `snapshot` (at the end, as clients.rs adds it); None when nothing differs."""
    if codex_snapshot(config) == snapshot:
        return None
    text = codex_without(config)
    if snapshot:
        newline = '\r\n' if '\r\n' in config else '\n'
        if text and not text.endswith('\n'):
            text += newline
        if text:
            text += newline
        text += newline.join(snapshot) + newline
    return text


# ---- both -------------------------------------------------------------------------------------------------------------
def snapshot(kind, path, home):
    """The bridge's entry in the file at `path` now (None values when the file is absent)."""
    path = Path(path)
    if not path.exists():
        return {'exists': False, 'entry': {'user': None, 'local': {}} if kind == 'claude' else None}
    text = path.read_bytes().decode('utf-8-sig')
    return {'exists': True, 'entry': claude_snapshot(text, home) if kind == 'claude' else codex_snapshot(text)}


def restore(kind, path, saved, home, tries=5):
    """Puts the bridge's entry in `path` back to `saved['entry']`, leaving every other entry as it is now. Returns what
    was done. A file another program rewrote while this ran is read again (a few times) instead of overwritten."""
    path = Path(path)
    for _ in range(tries):
        if not path.exists():
            return 'absent now: left absent'
        raw = path.read_bytes()
        bom = raw.startswith(b'\xef\xbb\xbf')
        text = raw.decode('utf-8-sig')
        try:
            merged = claude_merged(text, saved['entry'], home) if kind == 'claude' else codex_merged(text, saved['entry'])
        except ValueError:
            time.sleep(.2)    # caught half-written by its own program: read it again
            continue
        if merged is None:
            return 'unchanged'
        if not saved['exists'] and not merged.strip():
            # Only the bridge's block was ever in it: the file was created during the session for that block alone.
            if path.read_bytes() != raw:
                continue
            path.unlink()
            return 'removed (created during the session for the bridge entry only)'
        data = (b'\xef\xbb\xbf' if bom else b'') + merged.encode('utf-8')
        if path.read_bytes() != raw:
            continue      # another program wrote it meanwhile: merge into its version
        write_atomic(path, data)
        return 'bridge entry put back'
    raise RuntimeError(f'{path} kept changing while the bridge entry was put back; left as it is')
