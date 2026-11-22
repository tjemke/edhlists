"""Compare blind readings with the census, field by field.
usage: compare.py CENSUS_DIR OUT.jsonl [OUT2.jsonl ...]  (writes a report to stdout)"""
import json, os, sys
ORDER = 'WUBRGC'
def unit(u): return ''.join(sorted(u, key=lambda ch: ORDER.index(ch) if ch in ORDER else 9))
def norm(v, key=''):
    if isinstance(v, dict):
        return {k: norm(x, k) for k, x in v.items() if k not in ('note', 'unsure', 'name', 'cost')}
    if isinstance(v, list):
        xs = [norm(x, key) for x in v]
        if key in ('makes',): return sorted(unit(x) for x in xs)
        if key in ('types', 'untapped if you control', 'untapped if you reveal'): return sorted(str(x).lower() for x in xs)
        return xs
    if key == 'not_played' and v == 'Draw': return 'Card draw'
    return v
def flat(v, path='', out=None):
    out = {} if out is None else out
    if isinstance(v, dict):
        if not v: out[path] = '{}'
        for k, x in v.items(): flat(x, f'{path}.{k}' if path else k, out)
    elif isinstance(v, list) and key_is_struct(v):
        out[path + '#'] = len(v)
        for i, x in enumerate(v): flat(x, f'{path}[{i}]', out)
    else:
        out[path] = json.dumps(v)
    return out
def key_is_struct(v): return any(isinstance(x, dict) for x in v) or v == []
def diff(a, b):
    fa, fb = flat(norm(a)), flat(norm(b))
    return {k: (fa.get(k, '-'), fb.get(k, '-')) for k in sorted(set(fa) | set(fb)) if fa.get(k, '-') != fb.get(k, '-')}
def main():
    census = {}
    for l in open(os.path.join(sys.argv[1], 'cards.jsonl')):
        c = json.loads(l); census[c['name']] = c
    readers = {}
    for path in sys.argv[2:]:
        name = os.path.basename(path).removesuffix('.jsonl').removeprefix('out-')
        readers[name] = {}
        for i, l in enumerate(open(path)):
            if l.strip():
                try: r = json.loads(l); readers[name][r['name']] = r
                except Exception as e: print(f'{name}: line {i+1} unreadable: {e}')
    names = sorted(set().union(*readers.values()))
    stats = {}
    for n in names:
        c = census.get(n)
        if c is None: print(f'?? {n}: not in the census'); continue
        lines, agree_against = [], []
        for r, out in readers.items():
            if n not in out: lines.append(f'   {r}: missing'); continue
            d = diff(c['reading'], out[n])
            stats.setdefault(r, [0, 0]); stats[r][bool(d)] += 1
            if d:
                agree_against.append(r)
                lines.append(f'   census vs {r}: ' + '; '.join(f'{k}: {x} vs {y}' for k, (x, y) in d.items()))
                if out[n].get('note') or out[n].get('unsure'):
                    lines.append(f'      {r} note: {out[n].get("note","")} | unsure: {out[n].get("unsure","")}')
        rs = [r for r in readers if n in readers[r]]
        for i in range(len(rs)):
            for j in range(i + 1, len(rs)):
                d = diff(readers[rs[i]][n], readers[rs[j]][n])
                key = f'{rs[i]} vs {rs[j]}'; stats.setdefault(key, [0, 0]); stats[key][bool(d)] += 1
                if d: lines.append(f'   {key}: ' + '; '.join(f'{k}: {x} vs {y}' for k, (x, y) in d.items()))
        only_doubts = lines and all(('left_out:' in l and l.count(': ') == 2) or 'note:' in l for l in lines)
        if only_doubts:
            stats.setdefault('left_out only', [0, 0]); stats['left_out only'][1] += 1
            print(f"~~ {n}: left_out only: " + ' | '.join(l.strip() for l in lines if 'note:' not in l))
            continue
        if lines:
            tag = 'ALL READERS DIFFER FROM CENSUS' if agree_against and len(agree_against) == len(rs) else ''
            print(f"## {n} (rank {c['rank']}, {c['fidelity']}) {tag}")
            print('\n'.join(lines))
    print('\nagree / differ:')
    for k, (same, differ) in stats.items(): print(f'  {k}: {same} / {differ}')


if __name__ == '__main__':
    main()
