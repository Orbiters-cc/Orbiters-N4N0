// The VRChat bridge driven end to end by MCP's official TypeScript client, over the real transport, against the fake VRChat
// of `orbiters-desktop --bridge-lab`: nothing here touches the real game or this computer's desktop.
//
//   node scripts/bridge-lab.mjs <path to orbiters-desktop.exe> <folder with node_modules/@modelcontextprotocol/sdk>
//
// Exits 0 only when every check held. See docs/vrchat-bridge.md.
import { spawn } from 'node:child_process';
import { createSocket } from 'node:dgram';
import { createInterface } from 'node:readline';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [exe, sdkDir] = process.argv.slice(2);
if (!exe || !sdkDir) {
  console.error('usage: node scripts/bridge-lab.mjs <orbiters-desktop.exe> <folder with node_modules>');
  process.exit(2);
}
const sdk = (path) => import(pathToFileURL(resolve(sdkDir, 'node_modules/@modelcontextprotocol/sdk/dist/esm', path)).href);
const { Client } = await sdk('client/index.js');
const { StreamableHTTPClientTransport } = await sdk('client/streamableHttp.js');

let failed = 0;
const check = (ok, what, detail = '') => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok || !detail ? '' : `: ${detail}`}`);
  if (!ok) failed += 1;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** The stand-in for VRChat's OSC port, listening outside the lab: it hears the guard after the lab is killed. */
const osc = createSocket('udp4');
const heard = [];
osc.on('message', (message) => {
  const address = message.toString('latin1', 0, message.indexOf(0));
  const value = message.readInt32BE(message.length - 4);
  heard.push([address, value]);
});
await new Promise((r) => osc.bind(0, '127.0.0.1', r));

function startLab(dir) {
  const child = spawn(exe, ['--bridge-lab', dir, String(osc.address().port)], { stdio: ['pipe', 'pipe', 'inherit'] });
  const lines = createInterface({ input: child.stdout });
  const waiting = [];
  lines.on('line', (line) => waiting.shift()?.(JSON.parse(line)));
  const next = () => new Promise((r) => waiting.push(r));
  const say = async (command) => {
    const reply = next();
    child.stdin.write(`${command}\n`);
    return reply;
  };
  return next().then((ready) => ({ child, ready, say }));
}

const options = (token) => ({ requestInit: { headers: { Authorization: `Bearer ${token}` } } });
async function connect(ready) {
  const client = new Client({ name: 'official-sdk-client', version: '1.0.0' });
  await client.connect(new StreamableHTTPClientTransport(new URL(ready.address), options(ready.token)));
  return client;
}
const tool = async (client, name, args = {}) => {
  const result = await client.callTool({ name, arguments: args });
  return { error: result.isError === true, data: result.structuredContent, content: result.content };
};

const dir = mkdtempSync(join(tmpdir(), 'bridge-lab-'));
let lab = await startLab(dir);
check(lab.ready.address.startsWith('http://127.0.0.1:'), 'it listens on 127.0.0.1', lab.ready.address);

// ---- Over the real transport, with the official client
const client = await connect(lab.ready);
check(client.getServerVersion()?.name === 'orbiters-n4n0-vrchat-bridge', 'the official client connects and knows the server');
const { tools } = await client.listTools();
check(tools.length >= 10 && tools.length <= 12, `tools/list gives few tools (${tools.length})`);
const { prompts } = await client.listPrompts();
check(prompts[0]?.name === 'world_tour', 'the prompt is listed');
const guide = await client.readResource({ uri: 'bridge://guide' });
check(guide.contents[0].text.includes('When to give up'), 'the guide can be read');

// ---- Wrong token and Origin, through plain HTTP
const post = (headers) => fetch(lab.ready.address, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'ping' }) });
check((await post({ Authorization: 'Bearer wrong' })).status === 401, 'a wrong token gets 401');
check((await post({ Authorization: `Bearer ${lab.ready.token}`, Origin: 'https://evil.example' })).status === 403, 'another Origin gets 403');
check((await post({})).status === 401, 'no token gets 401');

// ---- The whole loop
const { world_a: worldA, world_b: worldB, camera_icon: icon } = lab.ready;
const found = await tool(client, 'find_worlds', { source: 'search', query: 'lab' });
check(found.data.worlds.length === 2, 'worlds are found');
for (const world of [worldA, worldB]) {
  await lab.say('foreground off');
  const went = await tool(client, 'go_to_world', { world_id: world });
  check(!went.error && went.data.arrived === true && went.data.private === true, `arrives in a private instance of ${world.slice(0, 13)}`, JSON.stringify(went.data));
  const shot = await tool(client, 'screenshot');
  check(shot.content[0].type === 'image' && shot.content[0].mimeType === 'image/jpeg' && shot.data.window_width === 640, 'a screenshot comes as an image with the window size', JSON.stringify(shot.data));
  // The real check on the real window says no: the fake window never has the focus.
  const refused = await tool(client, 'press', { input: 'esc' });
  check(refused.error && refused.data.code === 'not_foreground', 'a key is refused without the foreground', JSON.stringify(refused.data));
  await lab.say('foreground on');
  await tool(client, 'press', { input: 'esc' });
  await tool(client, 'mouse', { action: 'click', x: icon[0], y: icon[1] });
  await tool(client, 'press', { input: 'esc' });
  check((await tool(client, 'get_state')).data.camera.open === true, 'the camera opens through the menu');
  await tool(client, 'turn', { degrees: 30 });
  const walked = await tool(client, 'walk', { direction: 'forward', seconds: 0.3 });
  check(!walked.error, 'it walks', JSON.stringify(walked.data));
  const picture = await tool(client, 'take_3d_picture', { length_s: 4 });
  check(picture.data.saved === true && picture.data.reconstruction.state === 'waiting', 'a picture is saved and waits for VRChat to close', JSON.stringify(picture.data));
  await sleep(1100); // At most one world a second in the lab.
}
const closed = await tool(client, 'close_vrchat');
check(closed.data?.closed === true, 'VRChat closes', JSON.stringify(closed.data));
const finished = await tool(client, 'wait_for', { until: 'reconstructions_done', timeout_s: 10 });
check(finished.data.done === true, 'the reconstructions run once it is closed');
const pictures = (await tool(client, 'get_state')).data.pictures;
check(pictures.length === 2 && pictures.every((p) => p.reconstruction.state === 'done'), 'each picture ends reconstructed', JSON.stringify(pictures));

// ---- The person takes over
await tool(client, 'go_to_world', { world_id: worldA });
await lab.say('person');
const paused = await tool(client, 'walk', { direction: 'forward', seconds: 1 });
check(paused.error && paused.data.code === 'paused_by_person', 'the person pauses the agent', JSON.stringify(paused.data));
await lab.say('resume');

// ---- A client going away during a walk: released
await lab.say('foreground off');
heard.length = 0;
const aborter = new AbortController();
const going = fetch(lab.ready.address, {
  method: 'POST',
  signal: aborter.signal,
  headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${lab.ready.token}`, Host: new URL(lab.ready.address).host },
  body: JSON.stringify({ jsonrpc: '2.0', id: 9, method: 'tools/call', params: { name: 'walk', arguments: { direction: 'forward', seconds: 9 } } }),
}).catch(() => 'gone');
await sleep(800);
check(heard.some(([a, v]) => a === '/input/MoveForward' && v === 1), 'it walks', JSON.stringify(heard));
const cut = Date.now();
aborter.abort();
await going;
for (let i = 0; i < 40 && !heard.some(([a, v]) => a === '/input/MoveForward' && v === 0); i += 1) await sleep(50);
check(heard.at(-1)?.[0] === '/input/MoveForward' && heard.at(-1)?.[1] === 0, `the disconnect released it (in ${Date.now() - cut} ms)`, JSON.stringify(heard));

// ---- The app dying during a walk: the guard releases it
heard.length = 0;
const dying = fetch(lab.ready.address, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${lab.ready.token}` },
  body: JSON.stringify({ jsonrpc: '2.0', id: 10, method: 'tools/call', params: { name: 'walk', arguments: { direction: 'forward', seconds: 9 } } }),
}).catch(() => 'gone');
await sleep(800);
check(heard.some(([a, v]) => a === '/input/MoveForward' && v === 1), 'it walks again', JSON.stringify(heard));
const killed = Date.now();
lab.child.kill('SIGKILL');
await dying;
for (let i = 0; i < 60 && !heard.some(([a, v]) => a === '/input/MoveForward' && v === 0); i += 1) await sleep(50);
check(heard.some(([a, v]) => a === '/input/MoveForward' && v === 0), `the guard released it after the app was killed (in ${Date.now() - killed} ms)`, JSON.stringify(heard));

// ---- The flag
lab = await startLab(dir);
check(Boolean(lab.ready.token), 'it starts again');
await lab.say('owner off');
let port = (await lab.say('port')).port;
for (let i = 0; i < 80 && port !== null; i += 1) {
  await sleep(100);
  port = (await lab.say('port')).port;
}
check(port === null, 'the server saying no closes the port');
let refused = false;
try {
  await post({ Authorization: `Bearer ${lab.ready.token}` });
} catch {
  refused = true;
}
check(refused, 'nothing listens without the flag');
await lab.say('owner on');
for (let i = 0; i < 80 && port === null; i += 1) {
  await sleep(100);
  port = (await lab.say('port')).port;
}
check(port !== null, 'the server saying yes opens it again');
await lab.say('stop');
check((await lab.say('port')).port === null, 'Stop closes it at once');
await sleep(2000);
check((await lab.say('port')).port === null, 'and it stays closed after Stop');
lab.child.stdin.write('quit\n');
await new Promise((r) => lab.child.on('exit', r));
osc.close();
writeFileSync(join(dir, 'done'), '');
rmSync(dir, { recursive: true, force: true });
console.log(failed ? `${failed} FAILED` : 'all held');
process.exit(failed ? 1 : 0);
