"""SBX-2: a sandbox session puts back only the test bridge's entry in ~/.claude.json and ~/.codex/config.toml; whatever other
Claude Code / Codex sessions wrote during the session stays."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import sandbox_testbase  # first: every machine path into a temp folder
import client_configs as cc
import sandbox

HOME = 'C:\\Users\\someone'
OLD = {'type': 'http', 'url': 'http://127.0.0.1:4100/mcp', 'headers': {'Authorization': 'Bearer installed'}}
TEST = {'type': 'http', 'url': 'http://127.0.0.1:4999/mcp', 'headers': {'Authorization': 'Bearer test-build'}}


def claude(**top):
    return json.dumps(top, indent=2)


class Claude(unittest.TestCase):
    def test_only_the_bridge_entry_goes_back_and_others_writes_stay(self):
        start = claude(numStartups=1, mcpServers={'orbiters-vrchat': OLD, 'x': {'url': 'a'}}, projects={})
        saved = {'exists': True, 'entry': cc.claude_snapshot(start, HOME)}
        # During the session: the test build replaces the entry, another Claude session adds a project and a server.
        now = claude(numStartups=7, mcpServers={'x': {'url': 'a'}, 'orbiters-vrchat': TEST, 'added': {'url': 'b'}},
                     projects={'H:/repo': {'allowedTools': ['Bash']}})
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / '.claude.json'
            path.write_text(now)
            self.assertEqual(cc.restore('claude', path, saved, HOME), 'bridge entry put back')
            data = json.loads(path.read_text())
        self.assertEqual(data['mcpServers']['orbiters-vrchat'], OLD)
        self.assertEqual(data['numStartups'], 7)
        self.assertEqual(data['mcpServers']['added'], {'url': 'b'})
        self.assertEqual(data['projects'], {'H:/repo': {'allowedTools': ['Bash']}})
        self.assertEqual(list(data['mcpServers']), ['x', 'orbiters-vrchat', 'added'], 'order as the other program left it')

    def test_an_entry_absent_at_start_is_taken_away_also_from_the_home_project(self):
        start = claude(mcpServers={}, projects={'C:/Users/someone': {'mcpServers': {}}})
        saved = {'exists': True, 'entry': cc.claude_snapshot(start, HOME)}
        now = claude(mcpServers={'orbiters-vrchat': TEST}, projects={'C:/Users/someone': {'mcpServers': {'orbiters-vrchat': TEST}},
                                                                     'H:/other': {'mcpServers': {'orbiters-vrchat': TEST}}})
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / '.claude.json'
            path.write_text(now)
            cc.restore('claude', path, saved, HOME)
            data = json.loads(path.read_text())
        self.assertNotIn('orbiters-vrchat', data['mcpServers'])
        self.assertNotIn('orbiters-vrchat', data['projects']['C:/Users/someone']['mcpServers'])
        self.assertIn('orbiters-vrchat', data['projects']['H:/other']['mcpServers'], 'another folder\'s server is not the bridge\'s doing')

    def test_nothing_to_do_writes_nothing(self):
        text = claude(mcpServers={'orbiters-vrchat': OLD})
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / '.claude.json'
            path.write_bytes(text.encode())
            with patch.object(cc, 'write_atomic') as write:
                self.assertEqual(cc.restore('claude', path, {'exists': True, 'entry': cc.claude_snapshot(text, HOME)}, HOME), 'unchanged')
            write.assert_not_called()

    def test_a_write_racing_the_restore_is_merged_not_overwritten(self):
        saved = {'exists': True, 'entry': {'user': OLD, 'local': {}}}
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / '.claude.json'
            path.write_text(claude(mcpServers={'orbiters-vrchat': TEST}, n=1))
            real = cc.claude_merged
            def racing(text, *args):
                if not racing.done:   # another Claude session writes between our read and our write
                    racing.done = True
                    path.write_text(claude(mcpServers={'orbiters-vrchat': TEST}, n=2))
                return real(text, *args)
            racing.done = False
            with patch.object(cc, 'claude_merged', side_effect=racing):
                cc.restore('claude', path, saved, HOME)
            data = json.loads(path.read_text())
        self.assertEqual((data['n'], data['mcpServers']['orbiters-vrchat']), (2, OLD))

    def test_a_file_that_did_not_exist_is_never_deleted_when_others_wrote_into_it(self):
        saved = {'exists': False, 'entry': {'user': None, 'local': {}}}
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / '.claude.json'
            path.write_text(claude(mcpServers={'orbiters-vrchat': TEST}, firstStartTime='x'))
            cc.restore('claude', path, saved, HOME)
            self.assertEqual(json.loads(path.read_text()), {'mcpServers': {}, 'firstStartTime': 'x'})


class Codex(unittest.TestCase):
    MINE = '# mine\nmodel = "x"\n\n[mcp_servers.other]\nenabled = true\nargs = []\n'
    BLOCK = '[mcp_servers.orbiters-vrchat]\nurl = "{}"\nhttp_headers = {{ Authorization = "Bearer {}" }}\n'

    def test_others_lines_stay_and_the_old_block_comes_back(self):
        start = self.MINE + '\n' + self.BLOCK.format('http://a/mcp', 'installed')
        saved = {'exists': True, 'entry': cc.codex_snapshot(start)}
        now = self.MINE + '[projects."H:/repo"]\ntrust_level = "trusted"\n\n' + self.BLOCK.format('http://b/mcp', 'test')
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'config.toml'
            path.write_bytes(now.encode())
            cc.restore('codex', path, saved, HOME)
            text = path.read_bytes().decode()
        self.assertIn('[projects."H:/repo"]\ntrust_level = "trusted"\n', text)
        self.assertTrue(text.startswith(self.MINE))
        self.assertEqual(cc.codex_snapshot(text), cc.codex_snapshot(start))
        self.assertNotIn('Bearer test', text)

    def test_a_config_created_for_the_bridge_alone_goes_and_one_with_others_lines_stays(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'config.toml'
            path.write_text(self.BLOCK.format('u', 't'))
            cc.restore('codex', path, {'exists': False, 'entry': None}, HOME)
            self.assertFalse(path.exists())
            path.write_text('[projects."H:/repo"]\ntrust_level = "trusted"\n\n' + self.BLOCK.format('u', 't'))
            cc.restore('codex', path, {'exists': False, 'entry': None}, HOME)
            self.assertEqual(path.read_text(), '[projects."H:/repo"]\ntrust_level = "trusted"\n')

    def test_port_of_clients_rs_matches_its_own_test(self):
        rewritten = '[mcp_servers.orbiters-vrchat]\nurl = "u"\n\n[mcp_servers.orbiters-vrchat.http_headers]\nAuthorization = "Bearer t"\n\n# kept\n[mcp_servers.z]\nurl = "z"\n'
        self.assertEqual(cc.codex_without(rewritten), '# kept\n[mcp_servers.z]\nurl = "z"\n')
        self.assertEqual(cc.codex_without('[mcp_servers.orbiters-vrchat-2]\nurl = "z"\n'), '[mcp_servers.orbiters-vrchat-2]\nurl = "z"\n')

    def test_crlf_files_keep_crlf(self):
        start = 'a = 1\r\n\r\n' + self.BLOCK.format('u', 'old').replace('\n', '\r\n')
        now = 'a = 1\r\nb = 2\r\n\r\n' + self.BLOCK.format('u', 'new').replace('\n', '\r\n')
        out = cc.codex_merged(now, cc.codex_snapshot(start))
        self.assertEqual(out, 'a = 1\r\nb = 2\r\n\r\n' + self.BLOCK.format('u', 'old').replace('\n', '\r\n'))


class InSession(unittest.TestCase):
    """The whole journal path: backup at session start, someone else's writes during it, restore_contents at the end."""
    def run_session(self, test_app=True, legacy=False):
        root = sandbox.ROOT
        root.mkdir(parents=True, exist_ok=True)
        claude_path, codex_path = sandbox.HOME / '.claude.json', sandbox.HOME / '.codex/config.toml'
        claude_path.parent.mkdir(parents=True, exist_ok=True)
        claude_path.write_text(claude(mcpServers={'orbiters-vrchat': OLD}, n=1))
        codex_path.unlink(missing_ok=True)
        j = {'session': 's', 'files': [], 'processes': [], 'restored': False, 'shared': True}
        if test_app:
            j['app_test_exe'] = 'x.exe'
        with patch.multiple(sandbox, LOCK=root / 'lock', JOURNAL=root / 'journal'), patch.object(sandbox, 'processes', return_value=[]):
            if legacy:   # an older sandbox.py journalled the two files as whole bytes
                sandbox.backup(j, codex_path); sandbox.backup(j, claude_path)
            else:
                sandbox.client_backup(j, codex_path, 'codex'); sandbox.client_backup(j, claude_path, 'claude')
            sandbox.atomic_json(sandbox.JOURNAL, j)
            claude_path.write_text(claude(mcpServers={'orbiters-vrchat': TEST, 'someone': {'url': 'c'}}, n=2))
            codex_path.parent.mkdir(parents=True, exist_ok=True)
            codex_path.write_text('[projects."H:/r"]\ntrust_level = "trusted"\n\n[mcp_servers.orbiters-vrchat]\nurl = "u"\n')
            result = sandbox.restore()
        return result, json.loads(claude_path.read_text()), codex_path.read_text()

    def test_restore_keeps_other_sessions_writes(self):
        for legacy in (False, True):
            result, data, codex = self.run_session(legacy=legacy)
            self.assertTrue(result['restored'])
            self.assertEqual(data, {'mcpServers': {'orbiters-vrchat': OLD, 'someone': {'url': 'c'}}, 'n': 2}, legacy)
            self.assertEqual(codex, '[projects."H:/r"]\ntrust_level = "trusted"\n', legacy)

    def test_no_test_program_no_change(self):
        result, data, codex = self.run_session(test_app=False)
        self.assertEqual(data['mcpServers']['orbiters-vrchat'], TEST, 'the installed app may have written it (keep-app session)')
        self.assertIn('orbiters-vrchat', codex)

    def test_the_journal_holds_no_copy_of_the_files(self):
        j = {'files': []}
        path = sandbox.HOME / '.claude.json'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(claude(mcpServers={'orbiters-vrchat': OLD, 'other': {'headers': {'Authorization': 'Bearer someone-elses'}}}))
        sandbox.client_backup(j, path, 'claude')
        self.assertNotIn('someone-elses', Path(j['files'][0]['backup']).read_text())


if __name__ == '__main__':
    unittest.main()
