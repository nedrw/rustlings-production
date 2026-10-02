#!/usr/bin/env python3
"""lint-caps（文档级原型）

把 docs/curriculum.md 的「能力台账」（§3）与各 track 的步骤表机械对拍，检查：
  1. 步骤表声明的 teaches 必须在台账里有对应格位，且位置一致；
  2. 台账里的每个格位必须能在步骤表里反查到 teaches 声明；
  3. 同一 (能力, 深度) 不得被两个步骤重复教学（R1/R2）；
  4. 同一能力的深度必须沿路径严格递增、位置递增（R2）。

这是 M0 里 kiln lint-caps（Rust 版，读 track.toml + capabilities.toml）的原型：
先在文档阶段就把规则跑通，避免「机制只有承诺」。

用法：python3 tools/lint-caps-md.py   （在仓库根目录运行）
"""
import re, collections
full = open('docs/curriculum.md', encoding='utf-8').read()
ledger_doc = full.split('## 3. 能力台账')[1].split('## 4.')[0]
LEVELS = ['aware', 'apply', 'design', 'operate']
rank = {l: i for i, l in enumerate(LEVELS)}

def key(pos):                       # "3.14" -> (3, 14) 便于排序，同时保留原串用于打印
    a, b = pos.split('.'); return (int(a), int(b))

cell_re = re.compile(r'^T(\d)·(\d+\.\d+)$')
ledger = collections.defaultdict(dict)
for line in ledger_doc.splitlines():
    if not line.startswith('|'): continue
    cols = [c.strip() for c in line.strip().strip('|').split('|')]
    if len(cols) != 5: continue
    m = re.match(r'^`([a-z0-9-]+/[a-z0-9-]+)`$', cols[0])
    if not m: continue
    for lv, cell in zip(LEVELS, cols[1:]):
        mm = cell_re.match(cell.replace('`','').strip())
        if mm: ledger[m.group(1)][lv] = (f"T{mm.group(1)}·{mm.group(2)}", key(mm.group(2)), int(mm.group(1)))

teach_re = re.compile(r'`([a-z0-9-]+/[a-z0-9-]+)@(aware|apply|design|operate)`')
step_re  = re.compile(r'^(\d+)\.(\d+)$')
steps = collections.defaultdict(dict)
for line in full.splitlines():
    if not line.startswith('|'): continue
    cols = [c.strip() for c in line.strip().strip('|').split('|')]
    if len(cols) < 3: continue
    m = step_re.match(cols[0])
    if not m: continue
    tr, st = int(m.group(1)), f"{m.group(1)}.{m.group(2)}"
    for cap, lv in teach_re.findall(line):
        if steps[(tr, st)].get(cap) and rank[steps[(tr, st)][cap]] >= rank[lv]:
            print(f"  !! 同一步同能力多深度: T{tr}·{st} {cap}")
        steps[(tr, st)][cap] = lv

errs = []
for (tr, st), caps in sorted(steps.items()):
    for cap, lv in caps.items():
        got = ledger.get(cap, {}).get(lv)
        if got is None: errs.append(f"台账缺格: {cap}@{lv} 在 T{tr}·{st} 声明，台账 {lv} 列为空")
        elif got[0] != f"T{tr}·{st}": errs.append(f"台账不符: {cap}@{lv} 台账={got[0]}，步骤表=T{tr}·{st}")
seen = {}
for (tr, st), caps in steps.items():
    for cap, lv in caps.items():
        k = (cap, lv)
        if k in seen: errs.append(f"重复教学: {cap}@{lv} 同时出现在 T{seen[k][0]}·{seen[k][1]} 与 T{tr}·{st}")
        seen[k] = (tr, st)
for cap, levels in sorted(ledger.items()):
    for lv, (pos, _, tr) in levels.items():
        if steps.get((tr, pos.split('·')[1]), {}).get(cap) != lv:
            errs.append(f"反查失败: 台账 {cap}@{lv} = {pos}，但该步未声明此能力")
for cap, levels in sorted(ledger.items()):
    seq = sorted(levels.items(), key=lambda kv: (kv[1][2], kv[1][1]))
    for (l1, (p1, _, _)), (l2, (p2, _, _)) in zip(seq, seq[1:]):
        if rank[l2] <= rank[l1]: errs.append(f"深度倒退: {cap} {l1}@{p1} → {l2}@{p2}")
        elif (seq and (l1, p1) != (l2, p2)) and not (p2 > p1): errs.append(f"位置乱序: {cap} {l1}@{p1} → {l2}@{p2}")

print(f"台账条目 {len(ledger)}；步骤 {len(steps)}；台账声明的 (能力,深度) 格位 {sum(len(v) for v in ledger.values())}")
print(f"问题 {len(errs)} 条：")
for e in errs: print("  -", e)
