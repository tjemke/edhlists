"""Field-level diff of two census runs. usage: census_diff.py OLD_DIR NEW_DIR"""
import json, os, sys, collections
sys.path.insert(0, os.path.dirname(__file__))
from compare import diff
def load(d):
    out = {}
    for l in open(os.path.join(d, 'cards.jsonl')):
        c = json.loads(l); out[c['name']] = c
    return out
a, b = load(sys.argv[1]), load(sys.argv[2])
groups = collections.defaultdict(list)
for n in b:
    if n not in a: continue
    d = diff(a[n]['reading'], b[n]['reading'])
    fa, fb = a[n]['fidelity'], b[n]['fidelity']
    if d or fa != fb:
        key = (fb if fa != fb else '(same fidelity)')
        groups[key].append((b[n]['rank'] or 10**9, n, fa, d))
for key, items in sorted(groups.items(), key=lambda kv: -len(kv[1])):
    print(f'== {key} ({len(items)})')
    for rank, n, fa, d in sorted(items)[:int(os.environ.get('SHOW', 8))]:
        print(f'  {rank} {n} [was {fa[:40]}]: ' + '; '.join(f'{k}: {x} -> {y}' for k, (x, y) in d.items())[:260])
