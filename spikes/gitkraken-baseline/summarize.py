import json,sys,statistics as s
R=[json.loads(l) for l in open(sys.argv[1] if len(sys.argv)>1 else 'results/raw.jsonl')]
def q(v,p): v=sorted(v); return v[min(len(v)-1,int(p*len(v)))]
keys=sorted({(r.get('phase'),r.get('repo'),r.get('step') or r.get('dir') or r.get('passno')) for r in R if r.get('phase')},key=str)
for k in keys:
    x=[r for r in R if (r.get('phase'),r.get('repo'),r.get('step') or r.get('dir') or r.get('passno'))==k]
    if k[0]=='scroll':
        g=[a for r in x for a in r.get('gaps_ms',[])]
        print(k,'runs',len(x),'frames',len(g),'median',q(g,.5),'p95',q(g,.95),'p99',q(g,.99),'max',max(g) if g else None,'>8.4ms %.1f%%'%(100*sum(a>8.4 for a in g)/max(1,len(g))),'>17ms %.1f%%'%(100*sum(a>17 for a in g)/max(1,len(g))));continue
    if k[0]=='memory': print(k,x[-1].get('per_process'));continue
    o=[k,'n',len(x),'skip',sum(1 for r in x if r.get('skipped')),'err',sum(1 for r in x if 'error' in r)]
    for f in ('first_ms','settle_ms'):
        v=[r[f] for r in x if isinstance(r.get(f),(int,float))]
        if v:o+=[f,len(v),'med',round(s.median(v)),'p95',round(q(v,.95)),'min',round(min(v)),'max',round(max(v))]
    print(*o)
print('focus_lost',sum(1 for r in R if r.get('kind')=='focus_lost'))
