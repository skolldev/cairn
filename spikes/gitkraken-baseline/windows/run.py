#!/usr/bin/env python3
"""Drive GitKraken on Windows through the budget interactions, the same way ../run.py does on macOS."""
import csv, json, os, shutil, statistics, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
from locate import read_png  # noqa: E402

PROBE = os.path.join(HERE, 'target', 'release', 'probe.exe')
RES = os.path.join(HERE, '..', 'results', 'windows')
FX = 'C:/Users/guita/cairn-fixtures'
GK = os.path.expandvars(r'%LOCALAPPDATA%\gitkraken\gitkraken.exe')
PINS = {'linux-graph': '05441d697f7f9a927264f4058cbbc6a03dfcac9d',
        'linux-nograph': '05441d697f7f9a927264f4058cbbc6a03dfcac9d',
        'ripgrep': '91f5d22ef48bde459d90a8c7dbeab416e97d0f2b'}
ALL = {'linux-graph': 1505332, 'linux-nograph': 1505332, 'ripgrep': 2343}
KROWS = [3, 7, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 23, 24, 25, 27, 28, 29, 30]
ROWS = {'linux-graph': KROWS, 'linux-nograph': KROWS, 'ripgrep': list(range(1, 22))}
# Physical pixels on a maximized window, 3840x2160 at 150%. The Mac's points in ../run.py times about 1.5.
ROW0, PITCH, ROWX = 252, 42, 2000
PANEL = '3245,720,590,750'      # file list
CENTER = '1100,300,1100,1300'   # graph area for opens
DIFF = '800,300,1600,1300'      # diff view
BANNER = '3245,200,590,48'      # "N file changes"
TAGS = '250,595,60,30'          # tag count in the sidebar
SCROLL = '630,240,1425,1350'
LOG = open(os.path.join(RES, 'raw.jsonl'), 'a', buffering=1)


def sh(*a, **k): return subprocess.run(a, capture_output=True, text=True, **k)
def log(**k): k['at'] = time.strftime('%H:%M:%S'); LOG.write(json.dumps(k) + '\n')
def say(*a): print(time.strftime('%H:%M:%S'), *a, flush=True)
def p(*a): return sh(PROBE, *a).stdout.strip()
def activate(): p('activate'); time.sleep(.25)
def rowy(n): return ROW0 + PITCH * n
def key_esc(): p('key', '--vk', '0x1B')
def close_tab(): p('key', '--mod', '0x11', '--vk', '0x57')
def url(r): return f'gitkraken://repo/{FX}/{r}'
def openrepo(r): subprocess.run(['cmd', '/c', 'start', '', url(r)], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def front_ok():
    try: ok = json.loads(p('foreground'))['gitkraken']
    except Exception: ok = False
    if not ok:
        log(kind='focus_lost'); activate()


def check(strict_nograph=False):
    state = {}
    for r, pin in PINS.items():
        g = lambda *a: sh('git', '-C', f'{FX}/{r}', *a).stdout.strip()
        if g('rev-parse', 'HEAD') != pin or g('rev-parse', 'origin/master') != pin or int(g('rev-list', '--count', '--all')) != ALL[r]:
            raise SystemExit(f'PIN DRIFT in {r}')
        o = f'{FX}/{r}/.git/objects'
        state[r] = sorted(os.listdir(f'{o}/info')) + (['midx'] if os.path.exists(f'{o}/pack/multi-pack-index') else [])
    log(kind='graph_state', state=state)
    return state


def probe(*a):
    r = sh(PROBE, *a)
    try: return json.loads(r.stdout.strip().splitlines()[-1])
    except Exception: return {'error': r.stderr[-300:], 'stdout': r.stdout[-300:]}


def commits(repo, passno):
    for n in ROWS[repo]:
        front_ok()
        r = probe('watch', '--rect', PANEL, '--action', f'click:{ROWX},{rowy(n)}',
                  '--threshold', '0.001', '--quiet', '700', '--timeout', '6000')
        log(phase='commit', repo=repo, row=n, passno=passno, **r); time.sleep(.15)


LX, LY, LW, LH = 3245, 600, 85, 1000
def file_rows():
    """Pencil-icon bands in the commit panel, as y in physical pixels (see ../locate.py)."""
    f = os.path.join(HERE, 'loc.png')
    p('shot', '--rect', f'{LX},{LY},{LW},{LH}', '--out', f)
    w, h, bpp, px = read_png(f)
    ys = [y for y in range(h) if any(px[y][x*bpp] > 200 and px[y][x*bpp+1] > 140 and px[y][x*bpp+2] < 110 for x in range(w))]
    bands = []
    for y in ys:
        if bands and y - bands[-1][1] <= 4: bands[-1][1] = y
        else: bands.append([y, y])
    return [(a + b) / 2 + LY for a, b in bands if 9 <= b - a <= 24]


def files(repo):
    for n in ROWS[repo]:
        front_ok()
        sh(PROBE, 'watch', '--rect', PANEL, '--action', f'click:{ROWX},{rowy(n)}', '--quiet', '500', '--timeout', '3000')
        time.sleep(.2); b = file_rows()
        if len(b) < 2: log(phase='file', repo=repo, row=n, skipped=True, bands=b); continue
        r = probe('watch', '--rect', DIFF, '--action', f'click:3420,{b[-1]}', '--quiet', '800', '--timeout', '6000')
        log(phase='file', repo=repo, row=n, **r); key_esc(); time.sleep(.6)


def save_status(repo='linux-graph', n=12):
    """Needs a freshly opened tab: re-opening an open tab can leave updates dead (see RESULTS.md)."""
    path = f'{FX}/{repo}/README'; keep = open(path, 'rb').read(); size = len(keep)
    try:
        probe('watch', '--rect', PANEL, '--action', f'click:{ROWX},{rowy(3)}', '--quiet', '500', '--timeout', '3000'); time.sleep(.5)
        for i in range(n):
            for step, act in (('save', f'append:{path}'), ('revert', f'truncate:{size}:{path}')):
                r = probe('watch', '--rect', BANNER, '--action', act, '--threshold', '0.002', '--quiet', '1500', '--timeout', '8000')
                log(phase='status', repo=repo, step=step, i=i, **r)
    finally:
        open(path, 'wb').write(keep)


def git_ui(repo='ripgrep', n=10):
    g = f'{FX}/{repo}'
    try:
        for i in range(n):
            front_ok()
            for kind, cmd in (('tag', f'git -C {g} tag cairn-probe'), ('untag', f'git -C {g} tag -d cairn-probe')):
                r = probe('watch', '--rect', TAGS, '--action', 'exec:' + cmd, '--threshold', '0.005', '--quiet', '1500', '--timeout', '15000')
                log(phase='gitui', repo=repo, step=kind, i=i, **r)
    finally:
        sh('git', '-C', g, 'tag', '-d', 'cairn-probe')


def presentmon(process, seconds, name, fn):
    """Run fn while the elevated helper records presents, returning fn's result."""
    for f in ('pm.started', 'pm.done'):
        if os.path.exists(os.path.join(HERE, f)): os.remove(os.path.join(HERE, f))
    with open(os.path.join(HERE, 'pm.trigger'), 'w') as t: t.write(f'{process} {seconds} {name}\n')
    for _ in range(100):
        if os.path.exists(os.path.join(HERE, 'pm.started')): break
        time.sleep(.1)
    else: raise SystemExit('helper not responding')
    r = fn()
    for _ in range(200):
        if os.path.exists(os.path.join(HERE, 'pm.done')): return r
        time.sleep(.1)
    raise SystemExit('presentmon did not finish')


def intervals(name, t0, t1):
    """Display-change intervals inside [t0, t1] for the busiest swap chain in a PresentMon CSV."""
    rows = list(csv.DictReader(open(os.path.join(RES, name), encoding='utf-8-sig')))
    rows = [x for x in rows if t0 <= float(x['CPUStartQPCTimeInMs']) <= t1]
    by = {}
    for x in rows: by.setdefault((x['ProcessID'], x['SwapChainAddress']), []).append(x)
    if not by: return {'pm_error': 'no presents in window'}
    best = max(by.values(), key=len)
    num = lambda k: [round(float(x[k]), 2) for x in best if x[k] not in ('NA', '')]
    return {'pm_chains': len(by), 'pm_mode': sorted({x['PresentMode'] for x in best}),
            'pm_display_ms': num('MsBetweenDisplayChange'), 'pm_present_ms': num('MsBetweenPresents')}


def scroll(repo, n=5, delta=14):
    for i in range(n):
        front_ok()
        for d, s in (('down', -delta), ('up', delta)):
            name = f'pm-scroll-{repo}-{d}-{i}.csv'
            r = presentmon('gitkraken.exe', 5, name, lambda: probe('scroll', '--rect', SCROLL, '--at', '1900,900', '--delta', str(s), '--ticks', '500', '--every', '4'))
            log(phase='scroll', repo=repo, dir=d, i=i, **r, **intervals(name, r['t0_ms'], r['t1_ms'])); time.sleep(.5)


def control():
    name = 'pm-control.csv'
    r = presentmon('probe.exe', 8, name, lambda: probe('control', '--seconds', '5'))
    r.update(intervals(name, r['t0_ms'], r['t1_ms']))
    log(phase='control', **r)


def warm(repo, n=10):
    for i in range(n):
        front_ok(); close_tab(); time.sleep(2.5); front_ok()
        r = probe('watch', '--rect', CENTER, '--action', f'exec:start "" "{url(repo)}"', '--quiet', '4000', '--timeout', '40000')
        log(phase='warm', repo=repo, i=i, **r); time.sleep(.5)


def purge():
    t, d = os.path.join(HERE, 'cold.trigger'), os.path.join(HERE, 'cold.done')
    if os.path.exists(d): os.remove(d)
    open(t, 'w').close()
    for _ in range(600):
        if os.path.exists(d):
            return open(os.path.join(HERE, 'cold.result')).read().strip()
        time.sleep(.2)
    raise SystemExit('helper not responding')


def gone(): return json.loads(p('pids')) == []
def quit_gk():
    p('close')
    for _ in range(160):
        if gone(): return
        time.sleep(.25)
    raise SystemExit('GitKraken did not quit')
def launch_gk(): subprocess.run(['cmd', '/c', 'start', '', GK], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def strip_graph(repo):
    """Remove what GitKraken wrote, so each no-graph sample really has no commit-graph."""
    o = f'{FX}/{repo}/.git/objects'
    shutil.rmtree(f'{o}/info/commit-graphs', ignore_errors=True)
    for f in ('commit-graph',):
        if os.path.exists(f'{o}/info/{f}'): os.remove(f'{o}/info/{f}')
    for f in os.listdir(f'{o}/pack'):
        if f.startswith('multi-pack-index'): os.remove(f'{o}/pack/{f}')


def cold(repo, n):
    nograph = repo == 'linux-nograph'
    for i in range(n):
        quit_gk(); time.sleep(1)
        if nograph: strip_graph(repo)
        res = purge(); time.sleep(2)
        launch_gk(); time.sleep(9); activate()
        r = probe('watch', '--rect', CENTER, '--action', f'exec:start "" "{url(repo)}"', '--quiet', '15000', '--timeout', '70000')
        o = f'{FX}/{repo}/.git/objects'
        log(phase='cold', repo=repo, i=i, purge=res, after=sorted(os.listdir(f'{o}/info')), **r)
        activate(); close_tab(); time.sleep(1)


def launch(n=8):
    openrepo('ripgrep'); time.sleep(4)
    for i in range(n):
        quit_gk(); time.sleep(3)
        r = probe('watch', '--rect', '1300,600,1240,960', '--action', f'exec:start "" "{GK}"', '--threshold', '0.02', '--quiet', '5000', '--timeout', '45000')
        try: perf = json.load(open(os.path.expandvars(r'%APPDATA%\.gitkraken\perf-timings.json')))['milestoneTimings']
        except Exception as e: perf = str(e)
        log(phase='launch', i=i, perf=perf, **r); time.sleep(1)
        activate()


PS_MEM = r'''
$pids = (Get-Process gitkraken).Id
$ws = Get-CimInstance Win32_PerfRawData_PerfProc_Process | Where-Object { $pids -contains $_.IDProcess } |
  Select-Object IDProcess, WorkingSetPrivate, WorkingSet, PrivateBytes
$gpu = Get-CimInstance Win32_PerfRawData_GPUPerformanceCounters_GPUProcessMemory |
  Select-Object Name, DedicatedUsage, SharedUsage
@{ws = @($ws); gpu = @($gpu | Where-Object { $n = $_.Name; $pids | Where-Object { $n -like "pid_$($_)_*" } })} | ConvertTo-Json -Depth 3 -Compress
'''
def memory(repo='linux-graph', settle=20):
    activate(); openrepo(repo); time.sleep(settle)
    r = sh('powershell', '-NoProfile', '-Command', PS_MEM)
    d = json.loads(r.stdout)
    log(phase='memory', repo=repo, settle_s=settle, **d)
    say('memory MB', sum(x['WorkingSetPrivate'] for x in d['ws']) / 1e6, 'of', len(d['ws']), 'processes')


if __name__ == '__main__':
    say('state', check()); activate()
    for step in sys.argv[1:]:
        say('start', step); name, _, arg = step.partition(':')
        if name in ('commit', 'file', 'scroll', 'warm', 'open'):
            openrepo(arg); time.sleep(5 if arg == 'ripgrep' else 8); activate()
        if name == 'commit': commits(arg, 1); commits(arg, 2)
        elif name == 'file': files(arg)
        elif name == 'scroll': scroll(arg)
        elif name == 'control': control()
        elif name == 'warm': warm(arg)
        elif name == 'status':
            openrepo('linux-graph'); time.sleep(6); activate(); close_tab(); time.sleep(2.5)
            openrepo('linux-graph'); time.sleep(8); activate(); save_status()
        elif name == 'gitui': openrepo('ripgrep'); time.sleep(5); activate(); git_ui()
        elif name == 'launch': launch()
        elif name == 'cold': cold(arg.split('*')[0], int(arg.split('*')[1]))
        elif name == 'memory': memory()
        elif name == 'closefx':
            # Cold opens need no fixture tab restored at launch.
            for r in ('linux-graph', 'ripgrep'):
                openrepo(r); time.sleep(6); activate(); close_tab(); time.sleep(2)
        say('done', step)
    say('state', check()); say('all done')
