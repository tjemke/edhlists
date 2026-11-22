"""Agreement on draw between census runs and the blind readings, for Draw-tagged cards.
usage: draw_agree.py CENSUS_A CENSUS_B OUT.jsonl..."""
import json, sys, collections
def load(d): return {json.loads(l)['name']: json.loads(l) for l in open(f'{d}/cards.jsonl')}
A, B = load(sys.argv[1]), load(sys.argv[2])
agent = {}
for p in sys.argv[3:]:
    for l in open(p):
        if l.strip(): r = json.loads(l); agent[r['name']] = r
def kind(spell):
    if not spell: return 'none'
    e = spell['effect']
    if isinstance(e, dict):
        if 'draw' in e: return f"draw {e['draw']}"
        return list(e)[0]
    return e
n = sa = sb = 0; fixed = []; broke = []; still = []
for name, r in agent.items():
    if name not in B or 'Draw' not in B[name]['roles']: continue
    ka, kb, kg = kind(A[name]['reading']['spell']), kind(B[name]['reading']['spell']), kind(r.get('spell'))
    if 'rock' in (ka, kb, kg) or 'land_ramp' in (ka, kb, kg): continue
    n += 1; sa += ka == kg; sb += kb == kg
    if ka != kg and kb == kg: fixed.append((B[name]['rank'] or 10**9, name, ka, kb))
    if ka == kg and kb != kg: broke.append((B[name]['rank'] or 10**9, name, ka, kb))
    if ka != kg and kb != kg: still.append((B[name]['rank'] or 10**9, name, kb, kg))
print(f'{n} draw-tagged cards: old agrees {sa}, new agrees {sb}')
for label, xs in (('fixed', fixed), ('broke', broke), ('still differ (new vs agent)', still)):
    print(f'\n{label} ({len(xs)}):')
    for rank, name, x, y in sorted(xs): print(f'  {rank} {name}: {x} vs {y}')
