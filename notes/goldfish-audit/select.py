"""Pick the blind-reading population from the census and write agent inputs (blind fields only).
usage: select.py CENSUS_DIR OUT_DIR [--pilot N SEED] [--batches K]"""
import json, random, sys, os
census, out = sys.argv[1], sys.argv[2]
cards = [json.loads(l) for l in open(os.path.join(census, 'cards.jsonl'))]
def mana(c): return bool('Land' in c['type_line'] or c['produced'] or 'Ramp' in c['roles'])
def pick(c):
    r = c['rank'] or 10**9
    return (mana(c) and r <= 3800) or ('Draw' in c['roles'] and r <= 2000)
pop = [c for c in cards if pick(c)]
blind = lambda c: {k: c[k] for k in ('name', 'type_line', 'layout', 'mana_cost', 'oracle', 'produced', 'roles')}
print(len(pop), 'cards in the population;', sum(mana(c) for c in pop), 'mana,', sum(not mana(c) for c in pop), 'draw only')
os.makedirs(out, exist_ok=True)
if '--pilot' in sys.argv:
    i = sys.argv.index('--pilot'); n, seed = int(sys.argv[i+1]), int(sys.argv[i+2])
    random.seed(seed); sample = random.sample(pop, n)
    with open(os.path.join(out, 'input.jsonl'), 'w') as f:
        for c in sample: f.write(json.dumps(blind(c)) + '\n')
    print('pilot:', n, 'cards ->', out)
if '--batches' in sys.argv:
    k = int(sys.argv[sys.argv.index('--batches')+1])
    for b in range(k):
        with open(os.path.join(out, f'batch{b+1}.jsonl'), 'w') as f:
            for c in pop[b::k]: f.write(json.dumps(blind(c)) + '\n')
