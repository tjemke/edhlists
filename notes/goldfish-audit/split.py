"""Split a compare.py report into mana and draw disagreements. usage: split.py CMP.txt"""
import sys, re
blocks, cur = [], None
for line in open(sys.argv[1]):
    if line.startswith('## '): cur = [line.rstrip()]; blocks.append(cur)
    elif line.startswith('   ') and cur is not None: cur.append(line.rstrip())
    elif not line.startswith('   '): cur = None
mana_keys = re.compile(r'lands|rock|land_ramp|undercounted|"Ramp"|left_out')
draw, mana = [], []
for b in blocks:
    body = ' '.join(l for l in b[1:] if 'note:' not in l)
    (mana if mana_keys.search(body) else draw).append(b)
print(f'MANA ({len(mana)}):')
for b in mana: print('\n'.join(l[:300] for l in b))
print(f'\nDRAW ({len(draw)}): ' + '; '.join(b[0][3:].split(' (rank')[0] for b in draw))
