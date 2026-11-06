"""Reduce results/windows/raw.jsonl to the numbers in RESULTS-windows.md."""
import json, os, statistics as s
HERE = os.path.dirname(os.path.abspath(__file__))
RES = os.path.join(HERE, '..', 'results', 'windows')
R = [json.loads(l) for l in open(os.path.join(RES, 'raw.jsonl'))]
def q(v, p): v = sorted(v); return v[min(len(v) - 1, int(p * len(v)))]
def line(name, v):
    if not v: print(name, 'none'); return
    print(f'{name}: n={len(v)} median={s.median(v):.0f} min={min(v):.0f} max={max(v):.0f} p95={q(v, .95):.0f}')
def pick(phase, **k): return [r for r in R if r.get('phase') == phase and all(r.get(a) == b for a, b in k.items())]
def vals(x, f): return [r[f] for r in x if isinstance(r.get(f), (int, float))]
def gaps(name, g):
    if not g: print(name, 'none'); return
    print(f'{name}: n={len(g)} median={s.median(g):.1f} p95={q(g, .95):.1f} p99={q(g, .99):.1f} max={max(g):.1f} '
          f'>8.4={100*sum(a > 8.4 for a in g)/len(g):.1f}% >12.5={100*sum(a > 12.5 for a in g)/len(g):.1f}% >50={100*sum(a > 50 for a in g)/len(g):.1f}%')

for p in (1, 2):
    x = pick('commit', passno=p); line(f'commit pass {p} first', vals(x, 'first_ms')); line(f'commit pass {p} settled', vals(x, 'settle_ms'))
for r in ('linux-graph', 'ripgrep'):
    x = [a for a in pick('file', repo=r) if not a.get('skipped')]
    print('file skipped', r, len(pick('file', repo=r)) - len(x)); line(f'file {r} first', vals(x, 'first_ms')); line(f'file {r} settled', vals(x, 'settle_ms'))
x = pick('status'); print('status with no change', sum(1 for r in x if r.get('first_ms') is None))
line('status save', vals(pick('status', step='save'), 'first_ms')); line('status revert', vals(pick('status', step='revert'), 'first_ms'))
line('git tag', vals(pick('gitui', step='tag'), 'first_ms')); line('git untag', vals(pick('gitui', step='untag'), 'first_ms'))
x = pick('warm'); line('warm dimmed graph', vals(x, 'first_ms')); line('warm usable', vals(x, 'settle_ms'))
for repo in ('linux-graph', 'linux-nograph'):
    # The commits arrive as the change of about 0.08 after the loader; the last change settles them.
    x = pick('cold', repo=repo)
    line(f'cold {repo} loader', vals(x, 'first_ms'))
    line(f'cold {repo} first commits', [next(a[0] for a in r['series'] if a[1] >= .05 and a[0] > 3000) for r in x])
    line(f'cold {repo} settled', vals(x, 'settle_ms'))
x = pick('launch')
line('launch splash', vals(x, 'first_ms'))
line('launch main window', [next(a[0] for a in r['series'] if a[0] > 3000) for r in x])
line('launch settled', vals(x, 'settle_ms'))
line('launch app-reported splash', [m['timeSinceStartOrReload'] for r in x for m in r['perf'] if m['name'].startswith('Splash')])
x = pick('scroll')
print('scroll runs', len(x), 'chains', {r.get('pm_chains') for r in x}, 'modes', {tuple(r.get('pm_mode', [])) for r in x}, 'mean fps', round(sum(r['frames'] for r in x) / sum(r['seconds'] for r in x), 1))
gaps('scroll duplication', [a for r in x for a in r['gaps_ms']])
gaps('scroll PresentMon display', [a for r in x for a in r['pm_display_ms']])
x = pick('control')
print('control runs', len(x), 'mean fps', [round(r['frames'] / r['seconds'], 1) for r in x], 'PresentMon rows', [len(r.get('pm_display_ms', [])) for r in x])
gaps('control duplication', [a for r in x for a in r['gaps_ms']])
c = json.load(open(os.path.join(RES, 'calibrate.jsonl')))
line('click floor', c['ms']); print('click floor median', s.median(c['ms']))
for r in pick('memory'):
    print('memory private WS MiB', round(sum(w['WorkingSetPrivate'] for w in r['ws']) / 2**20), 'largest', round(max(w['WorkingSetPrivate'] for w in r['ws']) / 2**20),
          'working set MiB', round(sum(w['WorkingSet'] for w in r['ws']) / 2**20), 'processes', len(r['ws']), 'GPU dedicated MiB', round(sum(int(g['DedicatedUsage']) for g in r['gpu']) / 2**20))
print('focus_lost', sum(1 for r in R if r.get('kind') == 'focus_lost'), 'errors', sum(1 for r in R if 'error' in r))
