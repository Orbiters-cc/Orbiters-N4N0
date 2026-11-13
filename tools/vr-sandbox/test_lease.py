"""Turns on the shared sandbox session: lease rules, the controller's handling of them, and the EAC guards."""
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

import lease
import sandbox
import vrchat_life


class LeaseRules(unittest.TestCase):
    def test_first_caller_gets_the_turn_and_the_next_waits(self):
        s = lease.new_state(0)
        self.assertEqual(lease.acquire(s, 'a', 1)[0], 'granted')
        result, reason = lease.acquire(s, 'b', 2)
        self.assertEqual(result, 'wait')
        self.assertIn('a holds', reason)

    def test_reacquire_by_the_holder_keeps_its_token(self):
        s = lease.new_state(0)
        token = lease.acquire(s, 'a', 1)[1]
        self.assertEqual(lease.acquire(s, 'a', 5), ('granted', token))
        self.assertEqual(s['holder']['renewed'], 5)

    def test_turns_go_first_come_first_served(self):
        s = lease.new_state(0)
        token = lease.acquire(s, 'a', 0)[1]
        lease.acquire(s, 'b', 1)
        lease.acquire(s, 'c', 2)
        lease.end_turn(s, 3, 'released', 'a', token)
        self.assertEqual(lease.acquire(s, 'c', 4)[0], 'wait')      # b came first
        self.assertEqual(lease.acquire(s, 'b', 5)[0], 'granted')

    def test_a_waiter_that_stops_polling_loses_its_place(self):
        s = lease.new_state(0)
        token = lease.acquire(s, 'a', 0)[1]
        lease.acquire(s, 'b', 1)                                  # b never polls again
        lease.acquire(s, 'c', 2)
        later = 2 + lease.WAITER_STALE_S + 1
        lease.acquire(s, 'c', later - 1)
        lease.end_turn(s, later, 'released', 'a', token)
        self.assertEqual(lease.acquire(s, 'c', later)[0], 'granted')

    def test_only_the_holder_with_its_token_may_act(self):
        s = lease.new_state(0)
        token = lease.acquire(s, 'a', 0)[1]
        lease.check(s, 'a', token, 'pose', 10)
        self.assertEqual(s['holder']['renewed'], 10)
        for owner, tok in [('b', token), ('a', 'forged'), (None, None)]:
            with self.assertRaisesRegex(lease.LeaseError, 'Not your turn'):
                lease.check(s, owner, tok, 'pose', 11)
        self.assertEqual(s['holder']['renewed'], 10)

    def test_nobody_may_drive_without_a_turn_but_anyone_may_look(self):
        s = lease.new_state(0)
        with self.assertRaisesRegex(lease.LeaseError, 'take a turn first'):
            lease.check(s, 'a', 'x', 'input', 1)
        for action in ('status', 'acquire', 'yield'):
            lease.check(s, None, None, action, 1)

    def test_a_cut_off_holder_goes_stale_and_the_next_agent_takes_over(self):
        s = lease.new_state(0)
        lease.acquire(s, 'cut-off', 0)
        self.assertFalse(lease.stale(s['holder'], lease.STALE_S))
        self.assertTrue(lease.stale(s['holder'], lease.STALE_S + 1))
        now = lease.STALE_S + 1
        result, reason = lease.acquire(s, 'next', now)
        self.assertEqual(result, 'wait')
        self.assertIn('stale', reason)
        ended = lease.end_turn(s, now, 'taken over by next')
        self.assertEqual(ended['owner'], 'cut-off')
        self.assertEqual(lease.acquire(s, 'next', now)[0], 'granted')
        with self.assertRaisesRegex(lease.LeaseError, 'ended'):
            lease.renew(s, 'cut-off', ended['token'], now)
        self.assertEqual([h['event'] for h in s['history']], ['granted', 'taken over by next', 'granted'])

    def test_release_needs_the_holders_token(self):
        s = lease.new_state(0)
        token = lease.acquire(s, 'a', 0)[1]
        with self.assertRaises(lease.LeaseError):
            lease.end_turn(s, 1, 'released', 'a', 'wrong')
        self.assertIsNotNone(s['holder'])
        lease.end_turn(s, 1, 'released', 'a', token)
        self.assertIsNone(s['holder'])

    def test_idle_shutdown_counts_from_the_last_turn_only(self):
        s = lease.new_state(0)
        self.assertTrue(lease.idle_expired(s, lease.IDLE_S + 1))
        token = lease.acquire(s, 'a', 10)[1]
        self.assertFalse(lease.idle_expired(s, 10 + 10 * lease.IDLE_S))  # a held turn never idles out
        lease.end_turn(s, 100, 'released', 'a', token)
        self.assertFalse(lease.idle_expired(s, 100 + lease.IDLE_S))
        self.assertTrue(lease.idle_expired(s, 101 + lease.IDLE_S))

    def test_public_view_never_shows_the_token(self):
        s = lease.new_state(0)
        token = lease.acquire(s, 'a', 0)[1]
        lease.acquire(s, 'b', 1)
        view = lease.public(s, 2)
        self.assertNotIn(token, json.dumps(view))
        self.assertEqual(view['holder']['owner'], 'a')
        self.assertEqual(view['waiting'], ['b'])

    def test_owner_name_is_required(self):
        with self.assertRaises(lease.LeaseError):
            lease.acquire(lease.new_state(0), '', 0)


class FakeSession(sandbox.Session):
    """The controller's turn handling without SteamVR, VRChat or files."""
    def __init__(self, folder):
        super().__init__()
        self.j = {'session': 's', 'processes': [], 'files': []}
        self.shared = sandbox.Shared()
        self.mutex = 1
        self.ended = []
        self.stopped_apps = []

    def neutral(self, rejoin=True):
        self.ended.append(rejoin)
        return {'rejoined': rejoin}

    def vrchat(self):
        return None


class SessionTurns(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        root = Path(self.folder.name)
        self.patches = [patch.multiple(sandbox, ROOT=root, LEASES=root / 'leases', STATE=root / 'state.json',
                                       JOURNAL=root / 'journal.json', YIELD=root / 'YIELD', EAC_LOG=root / 'eac.jsonl'),
                        patch.object(sandbox, 'stop_app', side_effect=lambda app: 'quit' if app else None)]
        for p in self.patches:
            p.start()
        self.s = FakeSession(root)
        self.s.eac = vrchat_life.Log(root / 'eac.jsonl')

    def tearDown(self):
        for p in reversed(self.patches):
            p.stop()
        self.folder.cleanup()

    def ask(self, owner, action, **args):
        token = None
        try:
            token = json.loads(sandbox.owner_file(owner).read_text())['token']
        except OSError:
            pass
        return self.s.handle({'action': action, 'lease_owner': owner, 'lease_token': token, **args})

    def test_two_agents_take_turns_and_release_resets_to_neutral(self):
        self.assertTrue(self.s.handle({'action': 'acquire', 'owner': 'a'})['granted'])
        self.assertFalse(self.s.handle({'action': 'acquire', 'owner': 'b'})['granted'])
        with self.assertRaisesRegex(lease.LeaseError, 'Not your turn'):
            self.ask('b', 'menu', menu='close')
        self.s.lease['holder']['app'] = {'pid': 1, 'create_time': 1, 'exe': 'a.exe'}
        report = self.ask('a', 'release')
        self.assertEqual(report['app'], 'quit')
        self.assertEqual(self.s.ended, [True])
        self.assertFalse(sandbox.owner_file('a').exists())
        self.assertTrue(self.s.handle({'action': 'acquire', 'owner': 'b'})['granted'])
        self.assertFalse(self.s.stop.is_set())   # the session itself stays up

    def test_legacy_stop_from_the_holder_only_ends_its_turn(self):
        self.s.handle({'action': 'acquire', 'owner': 'a'})
        self.assertTrue(self.ask('a', 'stop')['released'])
        self.assertFalse(self.s.stop.is_set())

    def test_nobody_but_the_holder_can_shut_the_session_down_without_force(self):
        self.s.handle({'action': 'acquire', 'owner': 'a'})
        with self.assertRaisesRegex(lease.LeaseError, 'holds the sandbox'):
            self.ask('b', 'shutdown')
        with self.assertRaisesRegex(lease.LeaseError, 'Not your turn'):
            self.ask('b', 'stop')
        self.assertFalse(self.s.stop.is_set())
        self.ask('a', 'shutdown')
        self.assertTrue(self.s.stop.is_set())
        self.assertEqual(self.s.ended, [False])   # no rejoin when the whole session ends

    def test_a_stale_turn_is_taken_over_at_the_next_acquire(self):
        self.s.handle({'action': 'acquire', 'owner': 'cut-off'})
        self.s.lease['holder']['renewed'] -= lease.STALE_S + 5
        answer = self.s.handle({'action': 'acquire', 'owner': 'next'})
        self.assertTrue(answer['granted'])
        self.assertEqual(self.s.ended, [True])
        events = [e['event'] for e in self.s.lease['history']]
        self.assertIn('taken over by next', events)

    def test_the_tick_ends_a_stale_turn_then_idles_the_session_out(self):
        self.s.handle({'action': 'acquire', 'owner': 'a'})
        self.s.lease['holder']['renewed'] -= lease.STALE_S + 5
        with patch.object(sandbox, 'owner_wants_vr', return_value=None):
            self.s.tick()
            self.assertIsNone(self.s.lease['holder'])
            self.assertFalse(self.s.stop.is_set())
            self.s.lease['idle_since'] -= lease.IDLE_S + 1
            self.s.tick()
        self.assertTrue(self.s.stop.is_set())
        self.assertIn('idle', self.s.shutdown_reason)

    def test_the_owner_putting_his_headset_on_ends_the_session_after_a_short_hold(self):
        self.s.handle({'action': 'acquire', 'owner': 'a'})
        wants = "the owner's headset connected through Virtual Desktop"
        with patch.object(sandbox, 'owner_wants_vr', return_value=wants), patch.object(sandbox.time, 'time') as clock:
            clock.return_value = 1000
            self.s.tick()
            self.assertFalse(self.s.stop.is_set())
            clock.return_value = 1000 + sandbox.HEADSET_HOLD_S
            self.s.tick()
        self.assertTrue(self.s.stop.is_set())
        self.assertIn('headset', self.s.j['yielded'])

    def test_a_yield_file_ends_the_session_at_once(self):
        self.s.handle({'action': 'acquire', 'owner': 'a'})
        sandbox.YIELD.write_text('now')
        with patch.object(sandbox, 'headset_connected', return_value=False):
            self.s.tick()
        self.assertTrue(self.s.stop.is_set())
        self.assertFalse(sandbox.YIELD.exists())
        self.assertIsNone(self.s.lease['holder'])

    def test_requests_renew_the_turn(self):
        self.s.handle({'action': 'acquire', 'owner': 'a'})
        self.s.lease['holder']['renewed'] = 0
        with patch.object(sandbox.kernel, 'WaitForSingleObject', return_value=0), patch.object(sandbox.kernel, 'ReleaseMutex'):
            self.ask('a', 'menu', menu='close')
        self.assertGreater(self.s.lease['holder']['renewed'], 0)

    def test_the_app_logs_folder_may_never_be_the_owners(self):
        self.s.handle({'action': 'acquire', 'owner': 'a'})
        exe = sandbox.ROOT / 'test.exe'
        exe.write_bytes(b'')
        real = Path(sandbox.os.environ['LOCALAPPDATA']) / 'Orbiters' / 'logs'
        with self.assertRaisesRegex(ValueError, "owner's own"):
            self.ask('a', 'app', app_exe=str(exe), logs=str(real))


class TurnTokens(unittest.TestCase):
    def test_the_test_programs_environment_identifies_its_turn(self):
        with patch.dict(sandbox.os.environ, {'ORBITERS_VR_SANDBOX_OWNER': 'x', 'ORBITERS_VR_SANDBOX_LEASE': 't'}):
            self.assertEqual(sandbox.lease_args(), ('x', 't'))
        with tempfile.TemporaryDirectory() as folder, patch.object(sandbox, 'LEASES', Path(folder)):
            sandbox.owner_file('vr1113b/shell').write_text(json.dumps({'token': 'saved'}))
            with patch.dict(sandbox.os.environ, {}, clear=False):
                sandbox.os.environ.pop('ORBITERS_VR_SANDBOX_OWNER', None)
                self.assertEqual(sandbox.lease_args('vr1113b/shell'), ('vr1113b/shell', 'saved'))

    def test_render_sizes(self):
        self.assertEqual(sandbox.parse_render('2112x2304'), [2112, 2304])
        self.assertEqual(sandbox.parse_render('default'), sandbox.DEFAULT_RENDER)
        self.assertIsNone(sandbox.parse_render(None))
        with self.assertRaises(ValueError):
            sandbox.parse_render('100x100')

    def test_only_a_local_network_peer_counts_as_a_headset(self):
        self.assertTrue(sandbox.private_address('192.168.1.60'))
        self.assertTrue(sandbox.private_address('10.0.0.2'))
        self.assertFalse(sandbox.private_address('40.89.161.236'))   # VD's internet service, always connected
        self.assertFalse(sandbox.private_address('127.0.0.1'))


class AntiCheatGuards(unittest.TestCase):
    def snap(self, services=('STOPPED', 'STOPPED'), procs=()):
        return {'services': dict(zip(vrchat_life.EAC_SERVICES, ({'state': s} for s in services))), 'processes': list(procs)}

    def test_launch_waits_until_eac_stays_unloaded(self):
        busy = self.snap(('RUNNING', 'STOP_PENDING'), [{'name': 'EasyAntiCheat_EOS.exe', 'pid': 5}])
        states = iter([busy, busy, self.snap(), self.snap(), busy, self.snap()] + [self.snap()] * 50)
        clock = [0.0]
        def sleep(s): clock[0] += s
        with tempfile.TemporaryDirectory() as folder, patch.object(vrchat_life, 'snapshot', side_effect=lambda: next(states)):
            log = vrchat_life.Log(Path(folder) / 'log')
            waited = vrchat_life.wait_idle(log, 'test', timeout=60, margin=2, sleep=sleep, clock=lambda: clock[0])
            self.assertGreaterEqual(waited, 4)    # the busy blip reset the calm period
            self.assertIn('eac_idle', log.path.read_text())

    def test_launch_refuses_when_eac_never_unloads(self):
        busy = self.snap(('STOPPED', 'STOP_PENDING'))
        clock = [0.0]
        def sleep(s): clock[0] += s
        with tempfile.TemporaryDirectory() as folder, patch.object(vrchat_life, 'snapshot', return_value=busy), \
                patch.object(vrchat_life, 'driver_events', return_value=[]):
            log = vrchat_life.Log(Path(folder) / 'log')
            with self.assertRaisesRegex(RuntimeError, 'did not unload'):
                vrchat_life.wait_idle(log, 'test', timeout=10, margin=2, sleep=sleep, clock=lambda: clock[0])
            self.assertIn('eac_busy_timeout', log.path.read_text())

    def test_absent_services_count_as_idle(self):
        self.assertTrue(vrchat_life.idle(self.snap(('ABSENT', 'STOPPED'))))
        self.assertFalse(vrchat_life.idle(self.snap(procs=[{'name': 'launch.exe', 'pid': 1}])))

    def test_only_an_old_bootstrapper_window_without_vrchat_is_a_launch_error(self):
        procs = [{'name': 'start_protected_game.exe', 'pid': 1, 'ppid': 0, 'create_time': 100},
                 {'name': 'start_protected_game.exe', 'pid': 2, 'ppid': 0, 'create_time': 200},
                 {'name': 'start_protected_game.exe', 'pid': 3, 'ppid': 0, 'create_time': 300},
                 {'name': 'VRChat.exe', 'pid': 4, 'ppid': 2, 'create_time': 205}]
        with patch.object(vrchat_life, 'game_processes', return_value=procs), \
                patch.object(vrchat_life, 'windows_of', return_value=[{'hwnd': 9, 'title': 'VRChat', 'rect': [0, 0, 1, 1]}]):
            found = vrchat_life.error_windows(min_age_s=45, now=330)
        self.assertEqual([p['pid'] for p in found], [1])   # 2 started VRChat, 3 is still a fresh splash

    def test_join_is_read_from_the_vrchat_log_after_the_request(self):
        with tempfile.TemporaryDirectory() as folder:
            log = Path(folder) / 'output_log_x.txt'
            log.write_text('2026.11.13 12:00:00 Debug - [Behaviour] Joining wrld_a:1~private(u)\n'
                           '2026.11.13 12:00:05 Debug - [Behaviour] Finished entering world.\n'
                           '2026.11.13 12:10:00 Debug - [Behaviour] Joining wrld_a:1~private(u)\n')
            since = vrchat_life.time.mktime((2026, 11, 13, 12, 5, 0, 0, 0, -1))
            with patch.object(vrchat_life, 'vrchat_log_for', return_value=log):
                self.assertEqual(vrchat_life.joined(0, 'wrld_a:1~private(u)', since), (True, False))
                with log.open('a') as out:
                    out.write('2026.11.13 12:10:09 Debug - [Behaviour] Finished entering world.\n')
                self.assertEqual(vrchat_life.joined(0, 'wrld_a:1~private(u)', since), (True, True))


if __name__ == '__main__':
    unittest.main()
