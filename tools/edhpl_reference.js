const fs=require('fs');
// --- verbatim helpers from main-C1lbqCDd.js ---
const de=(t,s,o=1)=>{if(t<=s[0])return 0;if(t>s[s.length-1])return(s.length-1)*o;const n=s.map((p,f)=>({stop:f*o,max:p}));let m=0;for(let p=0;p<n.length-1;p++)if(t<n[p+1].max&&t>=n[p].max){m=Number(n[p].stop+(t-n[p].max)/(n[p+1].max-n[p].max));break}return m};
const factors={land:.6,reserved:.2,favorPrice:.25,powerCurve:[0,250,320,350,380,420,470,560,760,890,1e3],popCurve:[0,8500,13600,17100,19800,21900,23700,25300,26200,26700,27e3],priceCurve:[0,.5,1.5,3.5,6,10,15,25,40,65,100],cmcFloor:1.75,cmcCeiling:6,efficiencyLimits:[.65,1.1]};
const Ba=JSON.parse(fs.readFileSync(__dirname+'/edhpl_overrides.json','utf8'));
const BASICS=["Mountain","Forest","Island","Swamp","Plains","Wastes","Snow-Covered Mountain","Snow-Covered Forest","Snow-Covered Island","Snow-Covered Swamp","Snow-Covered Plains","Snow-Covered Wastes"];

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
module.exports = {de, score, factors};

if (require.main === module) {
  const decks = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
  console.log(JSON.stringify(decks.map(d => score(d)), null, 1));
}
