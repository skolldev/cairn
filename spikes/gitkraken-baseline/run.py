#!/usr/bin/env python3
"""Drive GitKraken through the budget interactions and log raw samples as JSONL."""
import json, os, subprocess, sys, time
import locate

HERE = os.path.dirname(os.path.abspath(__file__))
FX = os.path.expanduser('~/cairn-fixtures')
PINS = {'linux-graph': '05441d697f7f9a927264f4058cbbc6a03dfcac9d',
        'linux-nograph': '05441d697f7f9a927264f4058cbbc6a03dfcac9d',
        'ripgrep': '91f5d22ef48bde459d90a8c7dbeab416e97d0f2b'}
ALL = {'linux-graph': 1505332, 'linux-nograph': 1505332, 'ripgrep': 2343}
KROWS = [3, 7, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 23, 24, 25, 27, 28, 29, 30]
ROWS = {'linux-graph': KROWS, 'linux-nograph': KROWS, 'ripgrep': list(range(1, 22))}
LOG = open(os.path.join(HERE, 'results', 'raw.jsonl'), 'a', buffering=1)

def sh(*a): return subprocess.run(a, capture_output=True, text=True)
def log(**k): k['at'] = time.strftime('%H:%M:%S'); LOG.write(json.dumps(k) + '\n')
def say(*a): print(time.strftime('%H:%M:%S'), *a, flush=True)
def osa(s): return sh('osascript', '-e', s)
def activate():
    osa('tell application "GitKraken" to activate'); time.sleep(.25)
def front_ok():
    r = osa('tell application "System Events" to get name of first process whose frontmost is true')
    if r.stdout.strip() != 'GitKraken': log(kind='focus_lost', front=r.stdout.strip()); activate()
def key_esc(): osa('tell application "System Events" to key code 53')
def close_tab(): osa('tell application "System Events" to keystroke "w" using command down')
def openrepo(r): sh('open', f'gitkraken://repo{FX}/{r}')
def rowy(n): return 163 + 28 * n

def check():
    for r, pin in PINS.items():
        g = lambda *a: sh('git', '-C', f'{FX}/{r}', *a).stdout.strip()
        if g('rev-parse', 'origin/master') != pin or g('rev-parse', 'HEAD') != pin or int(g('rev-list', '--count', '--all')) != ALL[r]:
            raise SystemExit(f'PIN DRIFT in {r}')
    log(kind='graph_state', state={r: sorted(os.listdir(f'{FX}/{r}/.git/objects/info')) + (['midx'] if os.path.exists(f'{FX}/{r}/.git/objects/pack/multi-pack-index') else []) for r in PINS})

def probe(*a):
    r = sh(os.path.join(HERE, 'probe'), *a)
    try: return json.loads(r.stdout.strip().splitlines()[-1])
    except Exception: return {'error': r.stderr[-300:], 'stdout': r.stdout[-300:]}

def commits(repo, passno):
    for n in ROWS[repo]:
        front_ok()
        r = probe('watch', '--rect', '1383,480,340,500', '--action', f'click:1037,{rowy(n)}',
                  '--threshold', '0.001', '--quiet', '700', '--timeout', '6000')
        log(phase='commit', repo=repo, row=n, passno=passno, **r); time.sleep(.15)

def files(repo):
    for n in ROWS[repo]:
        front_ok()
        sh(os.path.join(HERE, 'probe'), 'watch', '--rect', '1383,480,340,500', '--action', f'click:1037,{rowy(n)}', '--quiet', '500', '--timeout', '3000')
        time.sleep(.2); b = locate.first_file_row()
        if len(b) < 2: log(phase='file', repo=repo, row=n, skipped=True, bands=b); continue
        r = probe('watch', '--rect', '500,200,700,700', '--action', f'click:1477,{b[-1]}', '--quiet', '800', '--timeout', '6000')
        log(phase='file', repo=repo, row=n, **r); key_esc(); time.sleep(.6)

def save_status(repo='linux-graph', n=12):
    """Needs a freshly opened tab: re-opening an open tab can leave updates dead (see notes)."""
    p = f'{FX}/{repo}/README'; keep = open(p, 'rb').read(); size = len(keep)
    def one(step, i, cmd):
        r = probe('watch', '--rect', '1383,130,345,28', '--action', 'exec:' + cmd, '--threshold', '0.002', '--quiet', '1500', '--timeout', '8000')
        log(phase='status', repo=repo, step=step, i=i, **r)
        return r.get('first_ms')
    try:
        probe('watch', '--rect', '1383,480,340,500', '--action', f'click:1037,{rowy(3)}', '--quiet', '500', '--timeout', '3000'); time.sleep(.5)
        for i in range(n):
            one('save', i, f'echo >> {p}')
            one('revert', i, f'head -c {size} {p} > {p}.t; mv {p}.t {p}')
    finally:
        open(p, 'wb').write(keep)

def git_ui(repo='ripgrep', n=10):
    g = f'{FX}/{repo}'
    try:
        for i in range(n):
            front_ok()
            for kind, cmd in (('tag', f'git -C {g} tag cairn-probe'), ('untag', f'git -C {g} tag -d cairn-probe')):
                r = probe('watch', '--rect', '340,419,50,24', '--action', 'exec:' + cmd, '--threshold', '0.005', '--quiet', '1500', '--timeout', '15000')
                log(phase='gitui', repo=repo, step=kind, i=i, **r)
    finally:
        sh('git', '-C', g, 'tag', '-d', 'cairn-probe')

def scroll(repo, n=5):
    for i in range(n):
        front_ok()
        for d, px in (('down', -12), ('up', 12)):
            r = probe('scroll', '--rect', '420,160,950,900', '--at', '1037,600', '--px', str(px), '--ticks', '500', '--every', '4')
            log(phase='scroll', repo=repo, dir=d, i=i, **r); time.sleep(.5)

def warm(repo, n=10):
    for i in range(n):
        front_ok(); close_tab(); time.sleep(2.5); front_ok()
        r = probe('watch', '--rect', '680,160,680,800', '--action', f"exec:open 'gitkraken://repo{FX}/{repo}'", '--quiet', '4000', '--timeout', '40000')
        log(phase='warm', repo=repo, i=i, **r); time.sleep(.5)

def purge():
    t = os.path.join(HERE, 'cold.trigger'); d = os.path.join(HERE, 'cold.done')
    if os.path.exists(d): os.remove(d)
    open(t, 'w').close()
    for _ in range(300):
        if os.path.exists(d): return
        time.sleep(.2)
    raise SystemExit('purge helper not responding')

def cold(repo, n):
    for i in range(n):
        if repo == 'linux-nograph' and os.path.exists(f'{FX}/{repo}/.git/objects/info/commit-graphs'):
            raise SystemExit('nograph has a commit-graph chain')
        quit_gk(); time.sleep(1); purge(); time.sleep(2)
        sh('open', '-a', 'GitKraken'); time.sleep(9); activate()
        r = probe('watch', '--rect', '680,160,680,800', '--action', f"exec:open 'gitkraken://repo{FX}/{repo}'", '--quiet', '15000' if repo == 'linux-nograph' else '4000', '--timeout', '70000')
        log(phase='cold', repo=repo, i=i, **r); activate(); close_tab(); time.sleep(1)

def gone():
    return sh('pgrep', '-x', 'GitKraken').stdout.strip() == ''
def quit_gk():
    osa('tell application "GitKraken" to quit')
    for _ in range(120):
        if gone(): return True
        time.sleep(.25)
    raise SystemExit('GitKraken did not quit')

def launch(n=8):
    openrepo('ripgrep'); time.sleep(4)
    for i in range(n):
        quit_gk(); time.sleep(3)
        r = probe('watch', '--rect', '0,33,1728,1084', '--action', 'exec:open -a GitKraken', '--threshold', '0.02', '--quiet', '5000', '--timeout', '45000')
        try: perf = json.load(open(os.path.expanduser('~/.gitkraken/perf-timings.json')))['milestoneTimings']
        except Exception as e: perf = str(e)
        log(phase='launch', i=i, perf=perf, **r); time.sleep(1)
        activate()

def memory(repo='linux-graph', settle=20):
    activate(); openrepo(repo); time.sleep(settle)
    pids = sh('pgrep', '-f', 'GitKraken').stdout.split(); rows = []
    for p in pids:
        o = sh('footprint', '-p', p).stdout
        for l in o.splitlines():
            if l.strip().startswith('phys_footprint:'): rows.append((p, l.split(':')[1].strip()))
    tabs = None
    log(phase='memory', repo=repo, settle_s=settle, per_process=rows, tabs=tabs)
    say('memory', rows)

PHASES = {'commit': lambda r: (commits(r, 1), commits(r, 2)), 'file': files}
if __name__ == '__main__':
    check(); activate()
    for step in sys.argv[1:]:
        say('start', step); name, _, arg = step.partition(':')
        if name in ('commit', 'file', 'scroll', 'warm', 'open'):
            openrepo(arg); time.sleep(5 if arg == 'ripgrep' else 8); activate()
        if name == 'commit': commits(arg, 1); commits(arg, 2)
        elif name == 'file': files(arg)
        elif name == 'scroll': scroll(arg)
        elif name == 'warm': warm(arg)
        elif name == 'status': openrepo('linux-graph'); time.sleep(6); activate(); close_tab(); time.sleep(2.5); openrepo('linux-graph'); time.sleep(8); activate(); save_status()
        elif name == 'gitui': openrepo('ripgrep'); time.sleep(5); activate(); git_ui()
        elif name == 'launch': launch()
        elif name == 'cold': cold(arg.split('*')[0], int(arg.split('*')[1]))
        elif name == 'memory': memory()
        check(); say('done', step)
    say('all done')
