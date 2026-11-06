// Regenerate tests/data/curve_golden.json from the JavaScript reference.
//
//     node tools/build_curve_fixture.js
//
// Emits every boundary value that matters (below the floor, on each stop, just past each stop,
// on and past the ceiling) plus a deterministic pseudo-random sample across each curve's range,
// so the Python port is pinned at exactly the places piecewise interpolation tends to go wrong.
const fs = require('fs');
const {de, factors} = require('./edhpl_reference.js');

const CURVES = {
  price: {curve: factors.priceCurve, step: 1 + factors.favorPrice},
  pop:   {curve: factors.popCurve,   step: 1 - factors.favorPrice},
  power: {curve: factors.powerCurve, step: 1},
};

let seed = 12345;
const rnd = () => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff;

const out = [];
for (const [name, {curve, step}] of Object.entries(CURVES)) {
  const max = curve[curve.length - 1];
  const values = [
    -5, 0, max, max + 1,                       // outside, and exactly on, the ends
    ...curve,                                  // exactly on every stop
    ...curve.map(x => x + 0.0001),             // a hair past every stop
    ...curve.slice(1).map(x => x - 0.0001),    // a hair below every stop
  ];
  for (let i = 0; i < 120; i++) values.push(rnd() * max * 1.2);
  for (const v of values) out.push([name, v, de(v, curve, step)]);
}

fs.writeFileSync(__dirname + '/../tests/data/curve_golden.json', JSON.stringify(out) + '\n');
console.log(`wrote tests/data/curve_golden.json: ${out.length} cases`);
