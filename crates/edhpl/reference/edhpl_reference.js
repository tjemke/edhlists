// EDHPowerLevel's scoring, transcribed from https://edhpowerlevel.com/js/main-C1lbqCDd.js
// (checked 2026-11-06). This is the ground truth the Rust port in ../src is tested against.
//
//     node reference/edhpl_reference.js tests/data/decks.json > tests/data/golden.json
//
// Input: [{file, cards: [DeckCard], combos: [Combo]}] — the shapes defined in ../src/lib.rs, which
// mirror the site's own card objects and its mapped Commander Spellbook combos (`rl()`).
// Everything between the "verbatim" markers is copied from the bundle; only `this.state.*`
// lookups were turned into function arguments.
const fs = require('fs');

// --- verbatim helpers ------------------------------------------------------------------------
const de=(t,s,o=1)=>{if(t<=s[0])return 0;if(t>s[s.length-1])return(s.length-1)*o;const n=s.map((p,f)=>({stop:f*o,max:p}));let m=0;for(let p=0;p<n.length-1;p++)if(t<n[p+1].max&&t>=n[p].max){m=Number(n[p].stop+(t-n[p].max)/(n[p+1].max-n[p].max));break}return m};
const factors={land:.6,reserved:.2,favorPrice:.25,powerCurve:[0,250,320,350,380,420,470,560,760,890,1e3],popCurve:[0,8500,13600,17100,19800,21900,23700,25300,26200,26700,27e3],priceCurve:[0,.5,1.5,3.5,6,10,15,25,40,65,100],bracketCurve:[0,4.7,6.7,7.7,9.25,10],cmcFloor:1.75,cmcCeiling:6,efficiencyLimits:[.65,1.1]};
const cardLists={massLandDenial:["Vorinclex, Voice of Hunger","Hall of Gemstone","Contamination","Cataclysm","Dimensional Breach","Epicenter","Global Ruin","Hokori, Dust Drinker","Razia's Purification","Rising Waters","Soulscour","Sunder","Apocalypse","Bearer of the Heavens","Conversion","Glaciers","Pox","Death Cloud","Tangle Wire","Restore Balance","Realm Razer","Spreading Algae","Numot, the Devastator","Kudzu","Demonic Hordes","Urza's Sylex","Infernal Darkness","Trinisphere","Worldfire","Worldslayer","Gilt-Leaf Archdruid","Worldpurge","Stasis"],MLDWhitelist:["Charitable Levy"],extraTurns:["Time Warp","Temporal Manipulation","Walk the Aeons","Capture of Jingzhou","Expropriate","Time Stretch","Nexus of Fate","Timestream Navigator","Sage of Hours","Lighthouse Chronologist","Time Sieve","Magosi, the Waterveil"]};
const Ee={producers:[7,4,3,6,17,77,60,79,325,110,505,1111,227,87,547,2742,2617],requirements:[28],prerequisites:[[/walking ballista has at least two \+1\/\+1 counters on it\./,4],[/you have a way to give (.+) (lifelink|indestructible|deathtouch|vigilance|persist|undying|trample|double strike|defender)\./]]};
// --- end verbatim ----------------------------------------------------------------------------

const Ba=JSON.parse(fs.readFileSync(__dirname+'/../data/overrides.json','utf8'));
const BASICS=["Mountain","Forest","Island","Swamp","Plains","Wastes","Snow-Covered Mountain","Snow-Covered Forest","Snow-Covered Island","Snow-Covered Swamp","Snow-Covered Plains","Snow-Covered Wastes"];
const ne=(arr,key,val)=>arr.findIndex(x=>x[key]===val);

// The site's per-card scoring (`te()` plus the aggregation that follows it), expression for expression.
function score(list){
  let H=0,w=0,g=0,J=0;
  let ba=[],da=[],ka=[],aa=[],ya=[],sa=[],va=[],Va=[];
  const ua=(Ta,ye,Ga)=>{if(Ta.length<Ga+1)for(;Ta.length<Ga+1;)Ta.push(0);Ta[Ga]=Ta[Ga]+ye};
  for(const T of list){
    const U={name:T.name,quantity:T.quantity};
    U.price=T.price;U.colors=T.colors;U.reserved=T.reserved;U.layout=T.layout;
    U.edhrec_rank=T.edhrec_rank;U.cmc=T.cmc;
    const o=Ba[U.name];
    if(o){ if(o.price!==undefined)U.price=U.price*o.price; if(o.cmc!==undefined)U.cmc=o.cmc; }
    if(T.reserved)U.price=U.price*factors.reserved;
    const pa=de(U.price,factors.priceCurve,1+factors.favorPrice);
    const fa=de(factors.popCurve[10]-U.edhrec_rank,factors.popCurve,1+factors.favorPrice*-1);
    U.impact=(pa+fa)*U.quantity;
    if(o){ if(o.impact!==undefined)U.impact=U.impact*o.impact;
           if(o.commanderImpact!==undefined&&T.isCommander)U.impact=U.impact*o.commanderImpact; }
    U.type=T.type_line.split(" // ")[0].split(" — ")[0];
    if(U.type.indexOf("Land")>-1||T.layout=="modal_dfc"){U.isLand=!0;U.impact=U.impact*factors.land;U.cmc=0}else U.isLand=!1;
    if(BASICS.includes(U.name))U.impact=2*U.quantity;
    if(U.layout!=="modal_dfc")H+=U.cmc*U.quantity;
    w+=U.impact;
    if(U.isLand){g+=U.quantity;ua(Va,U.impact,0);ua(ba,U.impact,0);}
    else{J+=U.quantity;ua(ba,U.impact,U.cmc);
      if(U.colors){
        if(U.colors.includes("R"))ua(da,U.impact/U.colors.length,U.cmc);
        if(U.colors.includes("W"))ua(ka,U.impact/U.colors.length,U.cmc);
        if(U.colors.includes("G"))ua(aa,U.impact/U.colors.length,U.cmc);
        if(U.colors.includes("U"))ua(ya,U.impact/U.colors.length,U.cmc);
        if(U.colors.includes("B"))ua(sa,U.impact/U.colors.length,U.cmc);
        if(U.colors.length===0)ua(va,U.impact,U.cmc);
      }}
  }
  const avgCost=Number(Number(H/J).toFixed(2));
  const pad=M=>{let z=[...M];for(;z.length<ba.length;)z.push(0);return z};
  const n=M=>!M.length>0?0:M.reduce((z,E)=>z+E,0);
  const m={cmcImpactR:pad(da),cmcImpactW:pad(ka),cmcImpactG:pad(aa),cmcImpactU:pad(ya),cmcImpactB:pad(sa),cmcImpactC:pad(va),
           cmcImpactTotalR:n(da),cmcImpactTotalW:n(ka),cmcImpactTotalG:n(aa),cmcImpactTotalU:n(ya),cmcImpactTotalB:n(sa),cmcImpactTotalC:n(va)};
  const p=m.cmcImpactTotalR+m.cmcImpactTotalW+m.cmcImpactTotalG+m.cmcImpactTotalU+m.cmcImpactTotalB+m.cmcImpactTotalC;
  let Ka=!1,q=0;
  ba.map((M,z)=>{if(Ka!==!1)return;const E=m.cmcImpactR[z]+m.cmcImpactW[z]+m.cmcImpactG[z]+m.cmcImpactU[z]+m.cmcImpactB[z]+m.cmcImpactC[z];q+=E,q>p*.65&&(Ka=z)});
  const gg=(avgCost+Ka)/2;
  const Ce=(factors.cmcCeiling-gg)/(factors.cmcCeiling-factors.cmcFloor);
  const Jf=(factors.efficiencyLimits[1]-factors.efficiencyLimits[0])*Ce;
  const X=factors.efficiencyLimits[0]+Jf;
  const ke=Number(w*X);
  const ie=de(ke,factors.powerCurve);
  return {level:ie,score:ke,impact:w,efficiency:X,avgCost,curveCmc:Ka===!1?0:Ka,lands:g,nonlands:J};
}

// The site's formattedList entries carry the post-override cmc, with lands/MDFCs at 0 (from `te()`).
function formatted(list){
  return list.map(T=>{
    const U={...T};
    const o=Ba[T.name];
    if(o&&o.cmc!==undefined)U.cmc=o.cmc;
    const type=T.type_line.split(" // ")[0].split(" — ")[0];
    if(type.indexOf("Land")>-1||T.layout=="modal_dfc")U.cmc=0;
    return U;
  });
}

// `bracketHelper(ie)`: `o` is the power level; `Ke` is the deck's Spellbook combos, already mapped
// by `rl()`. `this.state.formattedList` -> formattedList, `this.state.cardLists` -> cardLists.
function bracket(list, combos, o){
  const formattedList=formatted(list);
  let Ke=combos,na=0,xa=0;
  // --- verbatim -------------------------------------------------------------------------------
  const n={turns:{regex:/(take an extra turn|target player takes an extra turn|target player takes \w* extra turns)/,matches:[],maxes:[0,2,3,100,100],restricted:{cards:cardLists.extraTurns,underBracket:3,matches:[],passed:!1}},denial:{regex:/(^(noncreature|creature|red|white|blue|black|green) spells|^spells your opponents cast) cost {\d} more to cast|each player sacrifices \w* (lands|land for each)|destroy all (lands|islands|mountains|forests|swamps|plains)|destroy all (\w*, )*and lands|untap (only|more than) \w* (land|permanent|nonbasic)|(islands|mountains|forests|swamps|plains|\w* lands) don't untap|nonbasic lands are (mountains|islands)/,matches:[],maxes:[0,0,0,100,100],restricted:{cards:cardLists.massLandDenial,underBracket:3,matches:[]}},tutors:{regex:/search your library for (an*|up to \w*) (creature |artifact |enchantment |aura |equipment |planeswalker |instant |sorcery |permanent |\w* creature |\w* permanent)*(or \w* )*card/,matches:[],maxes:[2,3,100,100,100]},gameChangers:{matches:[],maxes:[0,0,3,100,100]},earlyCombos:{matches:[],maxes:[0,0,0,100,100]},lateCombos:{matches:[],maxes:[0,0,100,100,100]}};formattedList.forEach(B=>{B.oracle_text!==void 0&&B.oracle_text.forEach(H=>{const w=H.toLowerCase(),la=n.turns.regex.test(w),ba=n.denial.regex.test(w),da=formattedList[ne(formattedList,"name",B.name)].gamechanger;la&&!n.turns.matches.includes(B.name)&&n.turns.matches.push(B.name),ba&&!n.denial.matches.includes(B.name)&&!cardLists.MLDWhitelist.includes(B.name)&&n.denial.matches.push(B.name),da&&!n.gameChangers.matches.includes(B.name)&&n.gameChangers.matches.push(B.name),["earlyCombos","lateCombos","turns","denial","gameChangers"].map(aa=>{typeof n[aa].restricted<"u"&&n[aa].restricted.cards.includes(B.name)&&!n[aa].restricted.matches.includes(B.name)&&n[aa].restricted.matches.push(B.name)})})}),Ke.forEach((B,H)=>{if(typeof B>"u"){console.log("undefined combo at index",H);return}let w=B.cards.length,la=!1;B.produces.forEach(ya=>{Ee.producers.includes(ya.id)||(la=!0)});let ba=!1;B.requires.length<1&&(ba=!0),B.requires.forEach(ya=>{typeof ya.template<"u"&&typeof ya.quantity<"u"&&(w+=ya.quantity),Ee.requirements.includes(ya.template.id)||(ba=!0),w<3&&(ba=!1)});let da=!0,ka=0;if((B.notablePrerequisites?B.notablePrerequisites.split(`
`):[]).forEach(ya=>{Ee.prerequisites.forEach(sa=>{sa[0].test(ya.toLowerCase())&&(typeof sa[1]<"u"?ka+=sa[1]:da=!1)})}),la&&ba&&da&&w<3){let ya=0;B.cards.forEach(sa=>{const va=ne(formattedList,"name",sa.name);sa.zone.includes("B")&&(ya+=formattedList[va].cmc)}),B.mv+ya+ka>7?n.lateCombos.matches.push(B):n.earlyCombos.matches.push(B)}});let m=B=>{let H=0;return B.maxes.forEach((w,la)=>{B.matches.length>w&&(H=la+1)}),typeof B.restricted<"u"&&B.restricted.matches.length>0&&H<B.restricted.underBracket&&(H=B.restricted.underBracket),H};const p=["earlyCombos","lateCombos","turns","denial","gameChangers"].map(B=>m(n[B]));na=Math.max(...p);
  let X=5;o>0&&(X=Math.ceil(de(o,factors.bracketCurve))),xa=X>na+1?X:na+1;
  // --- end verbatim ---------------------------------------------------------------------------
  return {bracket:xa,floor:na+1,fromLevel:X,
          turns:n.turns.matches,turnsChainable:n.turns.restricted.matches,
          denial:n.denial.matches,denialListed:n.denial.restricted.matches,
          gameChangers:n.gameChangers.matches,
          early:n.earlyCombos.matches.map(c=>c.id),late:n.lateCombos.matches.map(c=>c.id)};
}

module.exports = {de, score, bracket, factors};

if (require.main === module) {
  const decks = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
  const out = {};
  for (const d of decks) {
    const s = score(d.cards);
    out[d.file] = {...s, ...bracket(d.cards, d.combos, s.level)};
  }
  console.log(JSON.stringify(out, null, 1));
}
