#!/usr/bin/env python3
"""Generate an interactive, DaisyDisk-like Cargo dependency report."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path
from typing import Any


SKIPPED_DIRECTORIES = {".git", "target"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path("tmp/cargo-dependencies/index.html"),
        help="output HTML file (default: %(default)s)",
    )
    parser.add_argument(
        "--metadata",
        type=Path,
        help="use an existing cargo metadata JSON file instead of running Cargo",
    )
    parser.add_argument(
        "--filter-platform",
        help="pass --filter-platform TARGET to cargo metadata",
    )
    return parser.parse_args()


def load_metadata(args: argparse.Namespace) -> dict[str, Any]:
    if args.metadata:
        return json.loads(args.metadata.read_text())

    command = ["cargo", "metadata", "--locked", "--format-version", "1"]
    if args.filter_platform:
        command.extend(["--filter-platform", args.filter_platform])
    return json.loads(subprocess.check_output(command))


def source_size(package: dict[str, Any]) -> tuple[int, int]:
    root = Path(package["manifest_path"]).parent
    byte_count = 0
    line_count = 0
    for directory, directories, files in os.walk(root):
        current = Path(directory)
        directories[:] = [
            name
            for name in directories
            if name not in SKIPPED_DIRECTORIES
            and not (current / name / "Cargo.toml").is_file()
        ]
        for name in files:
            source = current / name
            if source.suffix != ".rs":
                continue
            try:
                contents = source.read_bytes()
            except OSError:
                continue
            byte_count += len(contents)
            line_count += contents.count(b"\n") + bool(
                contents and not contents.endswith(b"\n")
            )
    return byte_count, line_count


def source_kind(package: dict[str, Any], workspace_members: set[str]) -> str:
    if package["id"] in workspace_members:
        return "workspace"
    source = package.get("source") or ""
    if source.startswith("registry+"):
        return "registry"
    if source.startswith("git+"):
        return "git"
    return "path"


def build_report_data(metadata: dict[str, Any], platform: str | None) -> dict[str, Any]:
    workspace_members = set(metadata["workspace_members"])
    workspace_root = Path(metadata["workspace_root"]).resolve()
    resolve_nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    packages = sorted(metadata["packages"], key=lambda package: package["id"])
    package_indexes = {package["id"]: index for index, package in enumerate(packages)}

    nodes = []
    for package in packages:
        byte_count, line_count = source_size(package)
        resolved = resolve_nodes.get(package["id"], {})
        dependencies: dict[int, dict[str, Any]] = {}
        for dependency in resolved.get("deps", []):
            target = package_indexes.get(dependency["pkg"])
            if target is None:
                continue
            edge = dependencies.setdefault(
                target, {"to": target, "names": [], "kinds": [], "targets": []}
            )
            edge["names"].append(dependency["name"])
            for dep_kind in dependency["dep_kinds"]:
                kind = dep_kind["kind"] or "normal"
                if kind not in edge["kinds"]:
                    edge["kinds"].append(kind)
                target_expression = dep_kind.get("target")
                if target_expression and target_expression not in edge["targets"]:
                    edge["targets"].append(target_expression)

        nodes.append(
            {
                "id": package["id"],
                "name": package["name"],
                "version": package["version"],
                "sourceKind": source_kind(package, workspace_members),
                "firstParty": Path(package["manifest_path"])
                .resolve()
                .is_relative_to(workspace_root),
                "sourceBytes": byte_count,
                "sourceLines": line_count,
                "features": resolved.get("features", []),
                "dependencies": sorted(dependencies.values(), key=lambda edge: edge["to"]),
            }
        )

    members = sorted(
        (package_indexes[member] for member in workspace_members),
        key=lambda index: nodes[index]["name"],
    )
    return {
        "generatedBy": "cargo metadata --locked",
        "platform": platform,
        "nodes": nodes,
        "workspaceMembers": members,
    }


HTML = r'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Golem crate dependencies</title>
<style>
:root { color-scheme: dark; --bg:#081018; --panel:#101b27; --line:#263646; --muted:#8fa4b8; --text:#ecf4fb; --accent:#5eead4; }
* { box-sizing: border-box; }
body { margin:0; background:radial-gradient(circle at 38% 35%,#13283a 0,#081018 48%,#050a10 100%); color:var(--text); font:14px/1.45 Inter,ui-sans-serif,system-ui,sans-serif; min-height:100vh; }
header { display:flex; align-items:flex-end; justify-content:space-between; gap:20px; padding:22px 28px 14px; border-bottom:1px solid #1c2c3a; background:#081018d9; backdrop-filter:blur(14px); position:sticky; top:0; z-index:5; }
h1 { font-size:21px; margin:0 0 3px; letter-spacing:-.02em; } header p { margin:0; color:var(--muted); }
.controls { display:flex; flex-wrap:wrap; justify-content:flex-end; gap:9px; }
select,input,button { color:var(--text); background:#132231; border:1px solid #304456; border-radius:7px; padding:8px 10px; font:inherit; }
select { max-width:240px; } input { width:180px; } button { cursor:pointer; } button:hover { border-color:var(--accent); }
main { display:grid; grid-template-columns:minmax(600px,1.65fr) minmax(340px,.8fr); min-height:calc(100vh - 86px); }
#map-wrap { position:relative; min-height:720px; overflow:hidden; }
#map { width:100%; height:calc(100vh - 88px); min-height:720px; display:block; }
#map path { stroke:#07111a; stroke-width:1.1; cursor:pointer; transition:opacity .15s,filter .15s; }
#map path:hover { filter:brightness(1.3); stroke:#bcecff; stroke-width:1.5; }
#map text { fill:#eaf6ff; font-size:10px; pointer-events:none; text-shadow:0 1px 2px #000,0 0 4px #000; }
#map text:not(.center-title):not(.center-sub) { display:none; }
.center-title { font-size:14px!important; font-weight:700; } .center-sub { fill:var(--muted)!important; font-size:10px!important; }
#crumbs { position:absolute; left:22px; top:18px; display:flex; gap:7px; align-items:center; max-width:calc(100% - 44px); flex-wrap:wrap; }
.crumb { padding:5px 8px; background:#0b1722d9; border:1px solid #2c4052; border-radius:999px; cursor:pointer; color:#bfd0df; }
.crumb:last-child { color:white; border-color:#4d708a; }
#tooltip { position:fixed; display:none; z-index:10; pointer-events:none; background:#06101aee; border:1px solid #46647b; border-radius:8px; padding:9px 11px; box-shadow:0 8px 30px #0008; max-width:300px; }
#tooltip strong { display:block; color:white; } #tooltip span { color:#a9bfd0; }
aside { border-left:1px solid #20303e; background:#0c151edb; padding:22px; overflow:auto; height:calc(100vh - 88px); }
.stats { display:grid; grid-template-columns:repeat(3,1fr); gap:8px; margin-bottom:18px; }
.stat { background:var(--panel); border:1px solid #213545; border-radius:9px; padding:11px; }
.stat b { display:block; font-size:19px; } .stat span { color:var(--muted); font-size:11px; }
h2 { font-size:15px; margin:20px 0 9px; } .detail { background:var(--panel); border:1px solid #213545; border-radius:9px; padding:13px; }
.detail h3 { margin:0 0 5px; font-size:16px; } .muted { color:var(--muted); } .path { font-family:ui-monospace,monospace; font-size:11px; overflow-wrap:anywhere; margin-top:8px; color:#b8d0e1; }
.map-help { color:var(--muted); font-size:11px; margin:8px 2px 0; } .ranking-wrap { border-bottom:1px solid #263746; } table { width:100%; border-collapse:collapse; font-size:11px; } th { color:var(--muted); text-align:right; font-weight:500; border-bottom:1px solid #2a3a48; padding:6px 5px; } th:first-child,td:first-child { text-align:left; } td { text-align:right; padding:6px 5px; border-bottom:1px solid #172735; } tr { cursor:pointer; } tbody tr:hover { background:#142635; } td:first-child { color:#d7eafa; overflow-wrap:anywhere; } #ranking-note { color:var(--muted); font-size:11px; margin-top:6px; } .row-dot { width:7px; height:7px; border-radius:50%; display:inline-block; margin-right:5px; }
.legend { display:flex; gap:12px; color:var(--muted); font-size:11px; margin-top:12px; flex-wrap:wrap; }.dot { width:8px;height:8px;border-radius:50%;display:inline-block;margin-right:4px; }
details { margin-top:18px; color:var(--muted); } summary { cursor:pointer; color:#bfd3e3; } details p { font-size:12px; }
.empty { fill:#7d93a4!important; font-size:14px!important; }
@media(max-width:1000px){ header{align-items:flex-start;flex-direction:column}.controls{justify-content:flex-start}main{grid-template-columns:1fr}aside{height:auto;border-left:0;border-top:1px solid #20303e}#map{height:760px} }
</style>
</head>
<body>
<header><div><h1>Golem crate dependencies</h1><p>Every wedge is a dependency gateway; its angle is the removable subtree behind it.</p></div>
<div class="controls">
<select id="root"></select>
<select id="scope"><option value="third-party">third-party only</option><option value="all">all crates</option></select>
<select id="kinds"><option value="normal">runtime dependencies</option><option value="build">runtime + build</option><option value="all">all, including dev</option></select>
<select id="metric"><option value="count">size by crate count</option><option value="bytes">size by Rust source</option></select>
<select id="impact"><option value="1">show every subtree</option><option value="2">collapse singletons</option><option value="5" selected>collapse subtrees &lt; 5</option><option value="10">collapse subtrees &lt; 10</option></select>
<input id="search" type="search" placeholder="Find a crate…">
<button id="reset" title="Return to the selected root">Reset zoom</button>
</div></header>
<main><section id="map-wrap"><svg id="map" viewBox="0 0 900 900" role="img" aria-label="Radial Cargo dependency dominator tree"></svg><div id="crumbs"></div></section>
<aside>
<div class="stats"><div class="stat"><b id="crate-count">–</b><span id="dependency-label">third-party dependencies</span></div><div class="stat"><b id="source-size">–</b><span>reachable Rust source</span></div><div class="stat"><b id="max-depth">–</b><span>dependency depth</span></div></div>
<div id="detail" class="detail"></div>
<div class="map-help">Angle = dominated subtree · gray = aggregated small subtrees · click a wedge or row to zoom</div>
<h2 id="ranking-title">Third-party entry crates by removal impact</h2>
<div class="ranking-wrap"><table><thead><tr><th>dependency</th><th title="Crates no longer reachable after removing this dependency edge">exclusive</th><th title="All crates reachable through this dependency">reach</th><th title="Reachable through other paths too">shared</th></tr></thead><tbody id="ranking"></tbody></table></div><div id="ranking-note"></div>
<div class="legend"><span id="workspace-legend"><i class="dot" style="background:#5eead4"></i>workspace</span><span><i class="dot" style="background:#60a5fa"></i>registry</span><span><i class="dot" style="background:#c084fc"></i>git/path</span><span><i class="dot" style="background:#425466"></i>collapsed</span></div>
<details><summary>How to read this</summary><p>In the default view all in-repository crates are contracted into the center. Its direct children are third-party crates declared anywhere in the selected Golem subtree. A crate is nested under another only if every third-party dependency path passes through that parent.</p><p>Small dominated subtrees are aggregated into gray wedges; lower the collapse threshold or search for a crate to reveal them. <b>Exclusive</b> is the exact number of third-party packages no longer reachable after removing every in-repository edge that introduces that entry crate. <b>Reach</b> includes packages also reachable through other entries.</p><p>Source size is uncompressed Rust source and is only a rough complexity proxy, not compile time or binary size. The default Cargo resolution includes all target conditions; use <code>--filter-platform</code> for one target.</p></details>
</aside></main><div id="tooltip"></div>
<script>const DATA=__REPORT_DATA__;
const svg=document.querySelector('#map'), rootSelect=document.querySelector('#root'), scopeSelect=document.querySelector('#scope'), kindsSelect=document.querySelector('#kinds'), metricSelect=document.querySelector('#metric'), impactSelect=document.querySelector('#impact'), search=document.querySelector('#search'), tooltip=document.querySelector('#tooltip');
const synthetic={name:'whole workspace',version:'',sourceKind:'workspace',firstParty:true,sourceBytes:0,sourceLines:0,features:[],dependencies:DATA.workspaceMembers.map(to=>({to,kinds:['normal'],names:[DATA.nodes[to].name],targets:[]}))};
const nodes=[...DATA.nodes,synthetic], syntheticIndex=nodes.length-1; let state=null, zoomIndex=null;
const fmt=new Intl.NumberFormat(); const formatBytes=n=>n<1024?`${n} B`:n<1048576?`${(n/1024).toFixed(1)} KiB`:`${(n/1048576).toFixed(1)} MiB`;
rootSelect.innerHTML=`<option value="${syntheticIndex}">Whole workspace</option>`+DATA.workspaceMembers.map(i=>`<option value="${i}">${nodes[i].name}</option>`).join('');
rootSelect.value=DATA.workspaceMembers.find(i=>nodes[i].name==='golem')??syntheticIndex;
function edgeAllowed(edge){return edge.kinds.some(k=>k==='normal'||(kindsSelect.value==='build'&&k==='build')||kindsSelect.value==='all')}
function adjacency(skipFrom=-1,skipTo=-1){return nodes.map((node,from)=>node.dependencies.filter(e=>edgeAllowed(e)&&!(from===skipFrom&&e.to===skipTo)).map(e=>e.to))}
function reachableFrom(root,adj){const seen=new Set([root]),parent=new Map(),depth=new Map([[root,0]]),queue=[root];for(let q=0;q<queue.length;q++){const from=queue[q];for(const to of adj[from])if(!seen.has(to)){seen.add(to);parent.set(to,from);depth.set(to,depth.get(from)+1);queue.push(to)}}return{seen,parent,depth}}
function scopedAdjacency(root,full){if(scopeSelect.value==='all')return{adj:full,introducers:new Map()};const internal=new Set([root]),queue=[root];for(let q=0;q<queue.length;q++){const from=queue[q];for(const to of full[from])if(nodes[to].firstParty&&!internal.has(to)){internal.add(to);queue.push(to)}}const introducers=new Map();for(const from of internal)for(const to of full[from])if(!nodes[to].firstParty){if(!introducers.has(to))introducers.set(to,[]);introducers.get(to).push(nodes[from].name)}const adj=nodes.map((node,index)=>node.firstParty?[]:full[index].filter(to=>!nodes[to].firstParty));adj[root]=[...introducers.keys()];return{adj,introducers}}
function popcount(value){let n=0;while(value){value&=value-1n;n++}return n}
function dominatorTree(root,adj,reach){const ids=[...reach.seen],all=ids.reduce((m,i)=>m|(1n<<BigInt(i)),0n),pred=nodes.map(()=>[]);for(const from of ids)for(const to of adj[from])if(reach.seen.has(to))pred[to].push(from);const dom=nodes.map(()=>0n);for(const i of ids)dom[i]=i===root?1n<<BigInt(root):all;let changed=true;while(changed){changed=false;for(const i of ids){if(i===root)continue;let next=all;for(const p of pred[i])next&=dom[p];next|=1n<<BigInt(i);if(next!==dom[i]){dom[i]=next;changed=true}}}const children=nodes.map(()=>[]),parent=new Map();for(const i of ids){if(i===root)continue;let strict=dom[i]&~(1n<<BigInt(i)),best=-1,bestRank=-1;for(const d of ids)if(strict&(1n<<BigInt(d))){const rank=popcount(dom[d]);if(rank>bestRank){best=d;bestRank=rank}}if(best>=0){children[best].push(i);parent.set(i,best)}}return{children,parent}}
function subtreeReach(start,adj,allowed){const seen=new Set([start]),queue=[start];for(let q=0;q<queue.length;q++)for(const to of adj[queue[q]])if(allowed.has(to)&&!seen.has(to)){seen.add(to);queue.push(to)}return seen}
function calculate(){const root=+rootSelect.value,scoped=scopedAdjacency(root,adjacency()),adj=scoped.adj,reach=reachableFrom(root,adj),dom=dominatorTree(root,adj,reach),entries=[...new Set(adj[root])];const direct=entries.map(index=>{const withoutAdj=adj.map(edges=>edges.slice());withoutAdj[root]=withoutAdj[root].filter(to=>to!==index);const without=reachableFrom(root,withoutAdj).seen,branch=subtreeReach(index,adj,reach.seen);let lost=0;for(const i of reach.seen)if(!without.has(i))lost++;return{index,lost,reach:branch.size,shared:branch.size-lost}}).sort((a,b)=>b.lost-a.lost||b.reach-a.reach);state={root,adj,reach,...dom,direct,introducers:scoped.introducers};zoomIndex=root;document.querySelector('#dependency-label').textContent=scopeSelect.value==='third-party'?'third-party dependencies':'dependencies (root excluded)';document.querySelector('#ranking-title').textContent=scopeSelect.value==='third-party'?'Third-party entry crates by removal impact':'Direct dependencies by removal impact';document.querySelector('#workspace-legend').style.display=scopeSelect.value==='third-party'?'none':'';render()}
function weight(i){return metricSelect.value==='bytes'?Math.max(nodes[i].sourceBytes,1):1}
function fullHierarchy(index){const children=state.children[index].map(fullHierarchy),own=weight(index),total=own+children.reduce((s,c)=>s+c.total,0),count=1+children.reduce((s,c)=>s+c.count,0),sourceBytes=nodes[index].sourceBytes+children.reduce((s,c)=>s+c.sourceBytes,0);return{index,children,own,total,count,sourceBytes}}
function collapseTree(tree,threshold){const children=tree.children.map(child=>collapseTree(child,threshold)),visible=children.filter(child=>child.count>=threshold),hidden=children.filter(child=>child.count<threshold);if(hidden.length){visible.push({index:null,hidden:true,children:[],own:0,total:hidden.reduce((s,c)=>s+c.total,0),count:hidden.reduce((s,c)=>s+c.count,0),sourceBytes:hidden.reduce((s,c)=>s+c.sourceBytes,0)})}return{...tree,children:visible}}
function hierarchy(index){return collapseTree(fullHierarchy(index),search.value?1:+impactSelect.value)}
function descendants(tree,out=[]){out.push(tree);for(const child of tree.children)descendants(child,out);return out}
function assign(tree,start,end,depth=0,branch=-1){tree.start=start;tree.end=end;tree.depth=depth;tree.branch=branch<0?tree.index:branch;const childTotal=tree.children.reduce((s,c)=>s+c.total,0);let cursor=start;for(const child of tree.children){const width=childTotal?(end-start)*child.total/childTotal:0;assign(child,cursor,cursor+width,depth+1,depth===0?child.index:tree.branch);cursor+=width}}
function polar(r,a){return[450+r*Math.sin(a),450-r*Math.cos(a)]}
function arcPath(a0,a1,r0,r1){if(a1-a0>=Math.PI*2-.0001)a1=a0+Math.PI*2-.0001;const p0=polar(r1,a0),p1=polar(r1,a1),p2=polar(r0,a1),p3=polar(r0,a0),large=a1-a0>Math.PI?1:0;return`M${p0} A${r1},${r1} 0 ${large},1 ${p1} L${p2} A${r0},${r0} 0 ${large},0 ${p3} Z`}
function color(tree){if(tree.hidden)return'#425466';const kind=nodes[tree.index].sourceKind,hues={workspace:168,registry:207,git:276,path:276};const base=hues[kind]??207,branchShift=(tree.branch*37)%48;return`hsl(${base+branchShift} ${Math.max(42,78-tree.depth*3)}% ${Math.min(68,43+tree.depth*2.5)}%)`}
function escapeHtml(s){return String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}
function render(){const tree=hierarchy(zoomIndex);assign(tree,0,Math.PI*2);const flat=descendants(tree),maxD=Math.max(1,...flat.map(n=>n.depth)),inner=74,ring=Math.min(100,(430-inner)/maxD);svg.innerHTML='';for(const item of flat){if(item.depth===0)continue;const r0=inner+(item.depth-1)*ring,r1=r0+ring-2,path=document.createElementNS('http://www.w3.org/2000/svg','path');path.setAttribute('d',arcPath(item.start,item.end,r0,r1));path.setAttribute('fill',color(item));if(!item.hidden){path.dataset.index=item.index;if(search.value&&!nodes[item.index].name.toLowerCase().includes(search.value.toLowerCase()))path.style.opacity='.13';path.addEventListener('click',()=>{zoomIndex=item.index;render()})}path.addEventListener('mousemove',event=>showTooltip(event,item));path.addEventListener('mouseleave',()=>tooltip.style.display='none');svg.append(path)}const title=document.createElementNS('http://www.w3.org/2000/svg','text');title.setAttribute('x',450);title.setAttribute('y',444);title.setAttribute('text-anchor','middle');title.setAttribute('class','center-title');title.textContent=nodes[zoomIndex].name;svg.append(title);const sub=document.createElementNS('http://www.w3.org/2000/svg','text');sub.setAttribute('x',450);sub.setAttribute('y',462);sub.setAttribute('text-anchor','middle');sub.setAttribute('class','center-sub');sub.textContent=`${fmt.format(Math.max(0,tree.count-1))} ${scopeSelect.value==='third-party'?'third-party ':''}dependencies`;svg.append(sub);renderCrumbs();showDetail(zoomIndex,tree);renderStats();renderRanking()}
function showTooltip(event,item){if(item.hidden){tooltip.innerHTML=`<strong>${fmt.format(item.count)} collapsed dependencies</strong><span>${formatBytes(item.sourceBytes)} Rust source<br>Lower the collapse threshold to inspect them.</span>`}else{const node=nodes[item.index];tooltip.innerHTML=`<strong>${escapeHtml(node.name)} ${escapeHtml(node.version)}</strong><span>${fmt.format(item.count)} crates in dominated subtree<br>${formatBytes(item.sourceBytes)} subtree source</span>`}tooltip.style.display='block';tooltip.style.left=Math.min(innerWidth-315,event.clientX+14)+'px';tooltip.style.top=Math.min(innerHeight-80,event.clientY+14)+'px'}
function renderCrumbs(){const chain=[];let i=zoomIndex;while(i!==undefined){chain.push(i);if(i===state.root)break;i=state.parent.get(i)}chain.reverse();document.querySelector('#crumbs').innerHTML=chain.map(i=>`<button class="crumb" data-index="${i}">${escapeHtml(nodes[i].name)}</button>`).join('<span class="muted">›</span>');document.querySelectorAll('.crumb').forEach(el=>el.onclick=()=>{zoomIndex=+el.dataset.index;render()})}
function shortestPath(index){const path=[index];while(index!==state.root&&state.reach.parent.has(index)){index=state.reach.parent.get(index);path.push(index)}return path.reverse().map(i=>nodes[i].name).join(' → ')}
function showDetail(index,tree){const node=nodes[index],introducedBy=state.introducers.get(index),introduction=introducedBy?`<div class="path">introduced by ${escapeHtml([...new Set(introducedBy)].sort().join(', '))}</div>`:'';document.querySelector('#detail').innerHTML=`<h3>${escapeHtml(node.name)} <span class="muted">${escapeHtml(node.version)}</span></h3><div class="muted">${fmt.format(tree.children.length)} visible wedges after collapsing · ${fmt.format(Math.max(0,tree.count-1))} dependencies / ${formatBytes(tree.sourceBytes)} reachable source</div>${introduction}<div class="path">${escapeHtml(shortestPath(index))}</div>`}
function renderStats(){const ids=[...state.reach.seen],bytes=ids.reduce((s,i)=>s+nodes[i].sourceBytes,0),depth=Math.max(...state.reach.depth.values());document.querySelector('#crate-count').textContent=fmt.format(ids.length-1);document.querySelector('#source-size').textContent=formatBytes(bytes);document.querySelector('#max-depth').textContent=fmt.format(depth)}
function entryColor(index){return`hsl(${207+(index*37)%48} 75% 45.5%)`}
function renderRanking(){const shown=Math.min(10,state.direct.length);document.querySelector('#ranking').innerHTML=state.direct.slice(0,shown).map(row=>`<tr data-index="${row.index}"><td title="${escapeHtml(nodes[row.index].name)} ${escapeHtml(nodes[row.index].version)}"><i class="row-dot" style="background:${entryColor(row.index)}"></i>${escapeHtml(nodes[row.index].name)}</td><td>${fmt.format(row.lost)}</td><td>${fmt.format(row.reach)}</td><td>${fmt.format(row.shared)}</td></tr>`).join('');document.querySelector('#ranking-note').textContent=`Top ${shown} of ${state.direct.length} ${scopeSelect.value==='third-party'?'third-party entry crates':'direct dependencies'}`;document.querySelectorAll('#ranking tr').forEach(el=>el.onclick=()=>{zoomIndex=+el.dataset.index;render()})}
rootSelect.onchange=calculate;scopeSelect.onchange=calculate;kindsSelect.onchange=calculate;metricSelect.onchange=render;impactSelect.onchange=render;search.oninput=render;document.querySelector('#reset').onclick=()=>{zoomIndex=state.root;render()};calculate();
</script></body></html>'''


def main() -> None:
    args = parse_args()
    metadata = load_metadata(args)
    report_data = build_report_data(metadata, args.filter_platform)
    encoded_data = json.dumps(report_data, separators=(",", ":")).replace("</", "<\\/")
    output = HTML.replace("__REPORT_DATA__", encoded_data)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(output)
    source_bytes = sum(node["sourceBytes"] for node in report_data["nodes"])
    print(
        f"wrote {args.out} with {len(report_data['nodes'])} packages "
        f"and {source_bytes / 1024 / 1024:.1f} MiB of Rust source"
    )


if __name__ == "__main__":
    main()
