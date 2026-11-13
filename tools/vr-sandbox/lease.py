"""Turns on the shared SteamVR/VRChat sandbox session: pure logic, no I/O, so it is unit-tested.

The controller (sandbox.py) keeps one long-lived session (SteamVR + VRChat). Agents take turns on it:

- One holder at a time. A turn is granted to the oldest live waiter (first come, first served).
- Every request the holder sends renews its turn. A turn with no request for STALE_S is stale: the controller ends
  it (stops that agent's program, puts the session back to neutral) and the next waiter gets the session.
  This is the takeover rule for an agent that was cut off mid-turn.
- A waiter must poll at least every WAITER_STALE_S, or it loses its place.
- With no holder for IDLE_S the session shuts itself down and restores the owner's SteamVR files.
"""
import os
import secrets

# The ORBITERS_SANDBOX_*_S overrides exist only to test these rules live in minutes; agents never set them.
STALE_S = int(os.environ.get('ORBITERS_SANDBOX_STALE_S', 10 * 60))   # a turn without any request for this long is ended
WAITER_STALE_S = 180       # a waiter that stopped polling for this long loses its place (a release can take 2 min)
IDLE_S = int(os.environ.get('ORBITERS_SANDBOX_IDLE_S', 10 * 60))     # no turn this long: the session shuts down
EXEMPT = {'status', 'lease_status', 'acquire', 'release', 'renew', 'shutdown', 'yield'}


class LeaseError(RuntimeError):
    pass


def new_state(now):
    return {'holder': None, 'waiters': {}, 'idle_since': now, 'history': []}


def _log(state, now, event, owner, **extra):
    state['history'] = (state['history'] + [dict(time=now, event=event, owner=owner, **extra)])[-200:]


def stale(holder, now):
    return holder is not None and now - holder['renewed'] > STALE_S


def live_waiters(state, now):
    """Waiters still polling, oldest first."""
    alive = {o: w for o, w in state['waiters'].items() if now - w['seen'] <= WAITER_STALE_S}
    state['waiters'] = alive
    return sorted(alive, key=lambda o: (alive[o]['since'], o))


def acquire(state, owner, now):
    """Ask for a turn. Returns ('granted', token) or ('wait', reason). A stale holder is NOT removed here: the caller
    ends that turn first (it has processes to stop) with end_turn, then asks again."""
    if not owner or not isinstance(owner, str) or len(owner) > 80:
        raise LeaseError('owner must be a short stream name, e.g. vr1113b/shell')
    holder = state['holder']
    if holder and holder['owner'] == owner:
        holder['renewed'] = now
        state['waiters'].pop(owner, None)
        return 'granted', holder['token']
    w = state['waiters'].setdefault(owner, {'since': now, 'seen': now})
    w['seen'] = now
    queue = live_waiters(state, now)
    if holder:
        if stale(holder, now):
            return 'wait', f"stale turn of {holder['owner']} is being ended"
        return 'wait', f"{holder['owner']} holds the session (position {queue.index(owner) + 1} in line)"
    if queue[0] != owner:
        return 'wait', f'{queue[0]} is first in line (position {queue.index(owner) + 1})'
    state['waiters'].pop(owner)
    state['holder'] = {'owner': owner, 'token': secrets.token_hex(16), 'since': now, 'renewed': now, 'app': None}
    state['idle_since'] = None
    _log(state, now, 'granted', owner)
    return 'granted', state['holder']['token']


def check(state, owner, token, action, now):
    """Raises unless the request may run now. Every accepted request renews the turn."""
    if action in EXEMPT:
        return
    holder = state['holder']
    if holder is None:
        raise LeaseError('Nobody holds the sandbox: take a turn first (python tools/vr-sandbox/sandbox.py acquire --owner <stream>)')
    if owner != holder['owner'] or token != holder['token']:
        who = owner or 'a caller without a turn'
        raise LeaseError(f"Not your turn: {holder['owner']} holds the sandbox ({who} asked for {action})")
    holder['renewed'] = now


def renew(state, owner, token, now):
    holder = state['holder']
    if not holder or holder['owner'] != owner or holder['token'] != token:
        raise LeaseError('Your turn has ended (released, expired after %d min without a request, or the owner took SteamVR back): acquire again' % (STALE_S // 60))
    holder['renewed'] = now


def end_turn(state, now, reason, owner=None, token=None):
    """Ends the current turn. With owner/token it must be that holder's (a release); without, it is the controller's
    decision (stale, yield, shutdown). Returns the ended holder or None."""
    holder = state['holder']
    if holder is None:
        return None
    if owner is not None and (holder['owner'] != owner or holder['token'] != token):
        raise LeaseError(f"Not your turn: {holder['owner']} holds the sandbox")
    state['holder'] = None
    state['idle_since'] = now
    _log(state, now, reason, holder['owner'], held_s=round(now - holder['since'], 1))
    return holder


def idle_expired(state, now):
    return state['holder'] is None and state['idle_since'] is not None and now - state['idle_since'] > IDLE_S


def public(state, now):
    """What any caller may see (never the token)."""
    h = state['holder']
    return {
        'holder': None if h is None else {'owner': h['owner'], 'since': h['since'], 'idle_s': round(now - h['renewed'], 1),
                                           'stale_in_s': round(max(0, STALE_S - (now - h['renewed'])), 1), 'app': h.get('app')},
        'waiting': live_waiters(state, now),
        'shutdown_in_s': None if state['idle_since'] is None or h else round(max(0, IDLE_S - (now - state['idle_since'])), 1),
        'rules': {'stale_turn_s': STALE_S, 'idle_shutdown_s': IDLE_S, 'waiter_poll_s': WAITER_STALE_S},
    }
