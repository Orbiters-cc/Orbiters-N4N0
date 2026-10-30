"""Windowless experiments for the 3D overlay, not a live headset test.

Use --suite timing, images, load or contention. Build the app and scripts/gpu-lab
first. Preserve a baseline executable before editing. All GPU cases run serially;
only the contention suite deliberately overlaps the renderer and a synthetic game.
"""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import subprocess
import time


def distribution(values):
    values = sorted(values)
    return {f'p{q}': values[int((len(values)-1)*q/100)] for q in [50, 95, 99]}


def main():
    ap = argparse.ArgumentParser(__doc__)
    ap.add_argument('--suite', choices=['timing', 'images', 'load', 'contention'], required=True)
    ap.add_argument('--exe', type=Path, required=True)
    ap.add_argument('--baseline', type=Path)
    ap.add_argument('--gpu-lab', type=Path)
    ap.add_argument('--capture', type=Path)
    ap.add_argument('--out', type=Path, required=True)
    ap.add_argument('--work-values', type=int, nargs='+', default=[800, 1800], help='Synthetic game shader iterations for contention')
    args = ap.parse_args()
    for k, v in vars(args).items():
        if isinstance(v, Path):
            setattr(args, k, v.resolve())
    args.out.mkdir(parents=True, exist_ok=True)
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)

    def run(name, command):
        p = args.out/name
        p.mkdir(exist_ok=True)
        with open(p/'run.log', 'w') as log:
            subprocess.run([str(x) for x in command], stdout=log, stderr=log, check=True, creationflags=flags)
        print(name, 'finished', flush=True)

    def render(name, extra, exe=None):
        run(name, [exe or args.exe, '--world-view', args.out/name, args.capture, *extra])

    meta = {k: str(v) for k, v in vars(args).items() if v is not None}
    meta['executable_sha256'] = hashlib.sha256(args.exe.read_bytes()).hexdigest()
    if args.capture:
        meta['capture_sha256'] = hashlib.sha256(args.capture.read_bytes()).hexdigest()
    (args.out/(args.suite+'-metadata.json')).write_text(json.dumps(meta, indent=2))
    if args.suite == 'timing':
        run('timing', [args.exe, '--world-timing', args.out/'timing', '3'])
    elif args.suite == 'images':
        # A/B then B/A for asymmetric sizes. Two independent symmetric runs.
        for name, extra in [('current-a', []), ('current-b', []),
                            ('asymmetric-old-a', ['--asymmetric', '--independent-eye-sizes']),
                            ('asymmetric-common-a', ['--asymmetric']),
                            ('asymmetric-common-b', ['--asymmetric']),
                            ('asymmetric-old-b', ['--asymmetric', '--independent-eye-sizes'])]:
            render(name, extra)
    elif args.suite == 'load':
        for quality in ['light', 'balanced', 'sharp']:
            render('load-'+quality, ['--load', '12', '--quality', quality])
        render('still', ['--load', '5', '--still'])
    else:
        if not args.baseline or not args.gpu_lab:
            ap.error('contention requires --baseline and --gpu-lab')
        rows = []
        for work in args.work_values:
            for order in range(2):
                for policy in (['alone', 'baseline', 'current'] if order == 0 else ['current', 'baseline', 'alone']):
                    name = f'contention-{work}-{order}-{policy}'
                    p = args.out/name
                    p.mkdir(exist_ok=True)
                    data = p/'game.csv'
                    with open(p/'game.log', 'w') as game_log:
                        game = subprocess.Popen([str(args.gpu_lab), 'load', '4080', '16', '--work', str(work),
                                                 '--paced', '90', '--feedback', '--out', str(data)],
                                                stdout=game_log, stderr=game_log, creationflags=flags)
                        try:
                            time.sleep(2)  # Deliberate warm-up inside a measured experiment.
                            if policy != 'alone':
                                exe = args.baseline if policy == 'baseline' else args.exe
                                extra = [] if policy == 'baseline' else ['--timing-feed', str(data)]
                                render(name, ['--load', '12', '--quality', 'balanced', *extra], exe)
                            if game.wait(timeout=60):
                                raise RuntimeError('gpu-lab failed: '+name)
                        finally:
                            if game.poll() is None:
                                game.terminate()
                                game.wait()
                    # Exclude both processes' setup and teardown, not slow frames inside the analysis window.
                    frames = [list(map(float, r)) for r in csv.reader(data.open()) if r]
                    frames = [r for r in frames if 5 <= r[0] <= 13]
                    result = {'name': name, 'work': work, 'policy': policy, 'n': len(frames),
                              'gpu_ms': distribution([r[1] for r in frames]),
                              'ready_from_scheduled_start_ms': distribution([r[3] for r in frames]),
                              'deadline_miss_percent': 100*sum(r[3] > 1000/90 for r in frames)/len(frames)}
                    if policy != 'alone':
                        result['overlay'] = json.loads((p/'load.json').read_text())
                    rows.append(result)
                    (args.out/'contention.json').write_text(json.dumps(rows, indent=2))
                    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    main()
