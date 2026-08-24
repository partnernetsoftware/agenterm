# `fleet.*` 目录与脚本侧绑定：漂移闸门 + 「生成还是手写」判决

| 字段 | 值 |
|------|-----|
| **文档** | `src/operations.rs::OPERATION_CATALOG` 与 `scripts/*/lib/fleet.*` 之间的一致性闸门，以及第三条绑定到来前的写法判决 |
| 日期 | 2026-08-25 |
| 状态 | 闸门已落地（`tests/fleet_catalog_conformance.rs`，9 测全绿）；判决为建议，未派单实施 |
| **产品真理** | [`prd/PRD_02_36_agenterm_qjswasm.md`](../prd/PRD_02_36_agenterm_qjswasm.md)（PRD 36）——第三条绑定是它的第一条归档门的锚 |
| 关联 | [`plan/design-agenterm-qjswasm.md`](design-agenterm-qjswasm.md)、`src/operations.rs`、`src/client/mod.rs`、`tests/script_fleet_facade_parity.rs` |
| 范围声明 | 本文不改任何绑定，不改 `src/**`。测量 + 判决 + 验收条件。修绑定另行派单 |

---

## 1. 一句话

**生成绑定的「上线层」，手写绑定的「顺手层」。** 今天手写的 29 个面里，**9 个（31%）
发出的参数对象是宿主会当场拒收的**——两条绑定错得一模一样。「手写是为了留下有意选择
的语法糖」这条辩护，在唯一一个被点名审视的例子（`fleet.tabs.set_note`）上不成立：那颗
糖不但可以机械推导，它本身就是那个 bug 的成因。

---

## 2. 谁是声明方

`src/operations.rs::OPERATION_CATALOG` 是声明方。每条 `OperationSpec` 同时带：

| 字段 | 例 | 谁在消费 |
|------|-----|---------|
| `script_surface` | `"fleet.tabs.set_note"` | 脚本侧绑定的函数路径 |
| `id` | `"tabs.set-note"` | `__host.fleet_call` 的第一个实参 |
| `parameters` | `[{name:"tab", required:true, …}, {name:"note", …}]` | `__host.fleet_call` 的第二个实参（JSON 对象的键） |

第三列过去没人对齐。`src/client/mod.rs:2649` 的 `validate_fleet_parameters` 会**逐键**校验：

```
if let Some(unknown) = object.keys().find(|name| !operation.parameters.iter().any(|spec| spec.name == *name)) {
    return Err(script_broker_error("broker_invalid_arguments",
        format!("{} does not accept parameter {unknown}", operation.id)));
}
```

而 `__host.fleet_call` 正是走到这里的：`src/script_worker.rs:616` 把 `(op_id, params_json)`
包成 broker 的 `"fleet.call"`，`src/client/mod.rs:2593` 的 `script_fleet_call` 接住，
`operation_by_id` 查目录，再进上面这段校验。所以**绑定的参数名不是文档口径，是线上契约**。

## 3. 实测（2026-08-25）

`tests/fleet_catalog_conformance.rs` 直接链 `agenterm::operations::OPERATION_CATALOG`
（Rust 侧不做文本解析），只把两个非 Rust 的绑定文件按文本解析，并在比对前先自证解析没坏
（`binding_parsers_see_every_definition_and_every_call`：解析到的定义数必须等于文件里
`call("` 的出现次数）。

| 量 | 值 |
|----|-----|
| `OPERATION_CATALOG` 条目 | **77**（44 条长写 + 33 条由 `nullary_ui_action()` 构造，`src/operations.rs:503`） |
| 其中 `fleet.*` 面 | 76（另 1 条是 `FleetTerminal.capture` → `pane.capture`，唯一的命名例外） |
| 两条绑定各实现 | **29**（覆盖 38%） |
| 没有任何绑定的面 | **47** |
| 绑定发明了目录没有的面 | **0** |
| 绑定转发了错的 operation id | **0** |
| **参数对象不合规的面** | **9 / 29（31%）** |
| lua 里重复赋值的命名空间表 | 9 处（`fleet.ui.tab = {}` 等各写两遍；js 移植时丢掉了，说明两文件是重敲不是共源） |

### 3.1 九条参数漂移

下面的载荷是**跑出来的**，不是正则猜的：用 `agenterm_lua::LuaEngine` 加一个会记录
`(op_id, params_json)` 的 `__host.fleet_call`，真执行 `scripts/lua/lib/fleet.lua`：

```
EMIT  tabs.set-note            {"note":"hello","tab_id":"@1"}
EMIT  ui.tab.select            {"id":"@2"}
EMIT  ui.input.wheel           {"delta":3}
EMIT  terminal.paste           {"text":"abc"}
EMIT  ui.composer.send         {"text":"hi"}
EMIT  ui.hello                 {}
EMIT  ui.deltas                {}
EMIT  events.read              {}
```

对上目录：

| 面 | 绑定发出 | 目录声明 | 性质 |
|----|---------|---------|------|
| `fleet.tabs.set_note` | `tab_id` | `tab`（required） | 键名错 + 缺必填 |
| `fleet.ui.tab.select` | `id` | `tab`（optional） | 键名错 |
| `fleet.ui.input.wheel` | `delta` | `x` / `y` / `delta_y` 三个 required | 键名错 + 缺三个必填 |
| `fleet.terminal.paste` | `text` | **无参数** | 键名错 |
| `fleet.ui.composer.send` | `text` | 只有 `tab` | 键名错 |
| `fleet.ui.hello` | `{}` | `minimum` / `maximum` required | 缺必填 |
| `fleet.ui.deltas` | `{}` | `epoch` / `after` required | 缺必填 |
| `fleet.events.read` | `{}` | `epoch` / `after` required | 缺必填 |
| `fleet.events.wait` | 只有 `timeout_ms` | 另有 `epoch` / `after` / `kind` required | 缺必填 |

> **口径**：载荷是实测事实；「因此宿主会回
> `broker_invalid_arguments`」是从 `src/client/mod.rs:2649` 那段校验**读出来的推论**，
> 不是端到端跑出来的——broker 路径要活服务器，本车道没起。要坐实它，实验是：起一个
> agenterm 服务器，`agenterm script` 跑一行 `fleet.tabs.set_note("@1","x")`，看返回是不是
> `tabs.set-note does not accept parameter tab_id`。这条实验没做，结论按推论记账。

### 3.2 「两条绑定 29/29 一致」证明不了正确性

`tests/script_fleet_facade_parity.rs::lua_and_qjs_fleet_facades_expose_identical_operation_catalogs`
是绿的，而且是真绿：两条绑定确实逐条相同。问题在于**它比的是抄得像不像，不是抄得对不对**。
上面九条漂移，两条绑定错得完全一致，所以互比看不见。

用变异测试把这件事钉死了。给 lua **和** js **同样**加一个目录没声明的参数
（`{ text = text, urgent = true }`）——正是「copy-and-compare」这套流程天然会产生的编辑：

```
### MUTATION 6: SAME extra param added to BOTH bindings (what copy-and-compare does)
test all_bindings_expose_the_same_surface_map ... ok          ← 互比：看不见
test binding_params_objects_conform_to_the_catalog_parameter_spec ... FAILED   ← 对目录比：抓到
```

### 3.3 顺带发现：一条静默的假绿

`tests/script_fleet_facade_parity.rs` 用扫 `src/operations.rs` 里 `id:` 行的办法取目录。
`nullary_ui_action()` 造出来的 33 条**没有 `id:` 行**，所以它只看见 77 条里的 44 条；
它的 sanity 检查是 `ids.len() >= 40`，44 照样过。后果是它的
`rh_surfaces_missing_from_host_catalog()` 把 33 个**其实已经在目录里、可派发**的操作
列为「宿主没实现」，测试全绿地断言了一句假话。

本闸门用 `catalog_length_is_literal_entries_plus_constructor_entries` 把这个机制钉住：
`长写条数 + nullary 条数 == OPERATION_CATALOG.len()`。这条不变式两种写法下都成立，
一旦出现第三种构造形状就会红，提醒任何按文本读目录的测试正在少数。

> **已修（2026-08-25，整合者）。** `host_operation_catalog_ids()` 改成直接 link
> `agenterm::operations::OPERATION_CATALOG`，不再扫文本。跑出来的真相比预计更干净：
> `rh_surfaces_missing_from_host_catalog()` 的正确内容是**空集**——rh 声明的 76 条
> `fleet.*` 全部在目录里、全部可派发，那 33 条「宿主没实现」从头到尾只存在于扫描器的
> 盲区里。
>
> ```text
> left:  {}
> right: {"terminal.copy-selection", "ui.font.decrease", … 33 条}
> ```
>
> 允许清单已清空，那条断言现在钉的是「空」，所以真出现一条派发不了的 rh surface 仍然会红。
> 这个数字曾经被抄进 `prd/PRD_02_10_rhai_scripting.md` 当作一条 open finding，
> 已同批撤销并写明为什么——用正则读源码的测试可以在**报告**里出错而不在断言里出错，
> 那种错是构造性静默的。

---

## 4. 判决：生成还是手写

### 4.1 先把辩方论据摆正

派单点名要看 `fleet.tabs.set_note` 怎么塑形参数再下结论。那就看：

```js
fleet.tabs.set_note = function (tabId, note) {
  return call("tabs.set-note", JSON.stringify({ tab_id: tabId, note: note }));
};
```

这里的「有意选择」有两层：

1. **位置参数而不是对象参数**——`set_note(tabId, note)` 比 `set_note({tab: …, note: …})` 顺手。
   这一层是真价值。但它**可以机械推导**：目录的 `parameters` 是有序数组，required 在前，
   `(tab, note)` 就是它的顺序。生成器给得出同样的签名。
2. **JS 侧形参叫 `tabId`（驼峰），JSON 键写成 `tab_id`（蛇形）**——这一层是纯手感，
   而且**它就是 bug**：JSON 键是照着 JS 形参名拍的，不是照着目录声明的 `tab` 拍的。
   目录从 0.1.9 起就写着 `name: "tab", value_type: "stable_tab_id"`。

所以在被点名的这个例子上，手写没有守住任何生成器给不了的东西，反而生产了漂移。
`fleet.ui.input.wheel(delta)` 更狠：一个标量参数对三个 required 参数，整条面不可用。

### 4.2 生成的真实代价（不粉饰）

| 代价 | 严重度 | 缓解 |
|------|--------|------|
| 多一个构建步骤 | 中 | 见 §4.4：产物签入仓库，不进 build.rs |
| 生成代码可读性差于手写 | 中 | 只生成上线层；顺手层仍手写（§4.3） |
| 三种目标语法（Lua 表 / JS 对象 / `.qjs` 自由函数无对象字面量） | 中 | 后端各一个 emitter，共用一份目录读取；`.qjs` 那份本来就没得抄（§5） |
| 可选参数的调用约定要一次性定死 | 中 | 生成器只给 required 位置参数 + 一个可选的 options 尾参；今天手写的做法（可选参数干脆不给）也是一种约定，不比它好 |
| 目录里 `value_type` 的语义（`stable_tab_id` 的 `@N` 形状）要不要在脚本侧预校验 | 低 | 不生成校验，宿主已经校验；生成器只保证键名和必填齐 |

### 4.3 判决与分层

**上线层（wire layer）生成，顺手层（sugar layer）手写。**

- **上线层** = `operation_id` 字符串 + 参数对象的键名 + 必填参数齐不齐。
  这一层**漂移是致命的、语法糖是零价值的**（没人会因为 JSON 键名好看而高兴），
  而且是今天九条 bug 的全部所在。生成。
- **顺手层** = 函数签名的形参名、参数顺序上的取舍、别名、doc 注释、
  「`set_note` 不需要显式传 tab 就默认当前 tab」这类便利。
  这一层**漂移是无害的、语法糖就是全部价值**。手写，写在生成产物**之上**，
  只允许调用生成出来的上线层函数，不允许自己拼 JSON。

这条分层同时回答了「生成的绑定会不会比手写的难读」：会，如果整份生成。
但整份生成不是必须的——把难读的那半藏在下面就行。

### 4.4 形态：签入的生成产物 + 「重新生成无 diff」测，不用 `build.rs`

理由不是审美，是加载路径：`crates/agenterm-lua/src/lib.rs:31-43` 的 `fleet_source()`
在**运行期**用 `env!("CARGO_MANIFEST_DIR")` 拼出 `scripts/lua/lib/fleet.lua` 再
`read_to_string`。绑定必须是磁盘上一个真文件；生成到 `OUT_DIR` 需要先改这个加载器，
是额外的、和本问题无关的风险。

所以：

1. 生成器是一个普通的 Rust 工具（`tools/` 下的 bin 或 `cargo test -- --ignored` 的一个
   写文件测试），读 `OPERATION_CATALOG`，写 `scripts/*/lib/fleet_generated.*`。
2. `tests/fleet_catalog_conformance.rs` 加一条：**重新生成一遍，与磁盘上的文件逐字节比**。
   不一致就红，红的修法是跑一次生成器。
3. 手写的顺手层是另一个文件，`require` / `import` 生成产物。
   本闸门现有的九条断言原样保留，作用不变：它们现在盯的是顺手层没有绕过上线层。

这样构建图不动，产物在 review 里可 diff、可 grep，闸门仍然是唯一的判据来源。

### 4.5 不生成什么

- `call()` 辅助函数本体、模块前言、结果解析（lua 的 `pcall` + `std.json.parse`，
  js 的 `JSON.parse` + 失败回退原串）。这些是**每引擎一次**的运行期事务，不随目录增长。
- `FleetTerminal.capture` / `pane.capture`。它的 `script_surface` 是伪类路径而不是
  `fleet.*` 点分路径，按现有命名约定生成器造不出对应函数。这条要么改目录给它一个
  `fleet.*` 面，要么明确它不属于脚本面——**改 `src/operations.rs` 不在本车道域内，
  报给集成者**。闸门里 `only_one_script_surface_sits_outside_the_fleet_namespace`
  已经把「只许有这一个例外」钉住了。

---

## 5. 第三条绑定 `.qjs`：为什么它把天平压死

PRD 36 §「第一条门的实测缺口清单」已实测：`.qjs` 今天**编不了对象字面量、编不了属性
访问、编不了把函数当值**（`prd/PRD_02_36_agenterm_qjswasm.md:280-282`）。
所以第一版 `.qjs` 绑定只能是自由函数：

```
fleet_tabs_list()
fleet_tabs_set_note(tab, note)
```

两条推论，都指向生成：

1. **它不是任何东西的移植。** 「copy-and-compare」这套维护流程的前提是有一份形状相同的
   源可抄。自由函数 + 无对象字面量，和 `fleet.js` 形状不同，**没得抄**。
   人要凭目录重新手打 76 个面——那正是今天 29 个面里错了 9 个的那道工序，样本量放大 2.6 倍。
2. **没有对象字面量意味着参数 JSON 得靠字符串拼。**
   `'{"tab":"' + tab + '","note":"' + note + '"}'`。手写 76 遍这种拼接，
   等于手写 76 遍转义：一条含 `"` 或反斜杠的 note 就能把 JSON 拼坏。
   生成器发一份共享的 `__json_escape` 再统一调用，这一类 bug 一次性归零。
   这一条是**生成独有**的收益，手写拿不到。

换句话说：前两条绑定「手写还是生成」是成本权衡；第三条绑定手写是**明知会重犯已知错误**。

---

## 6. 验收条件（可证伪）

采纳本判决，实施完成的判据：

1. `scripts/lua/lib/fleet_generated.lua` 与 `scripts/qjs/lib/fleet_generated.js`
   覆盖 `OPERATION_CATALOG` 全部 76 个 `fleet.*` 面（不是 29 个）。
2. `tests/fleet_catalog_conformance.rs::expected_parameter_drift()` **返回空 map**，
   且删掉这个函数后测试仍绿。
3. `unimplemented_surfaces()` **返回空集**，且删掉后测试仍绿。
4. 新增「重新生成无 diff」测为绿。
5. 顺手层里没有任何一处自己拼 `operation_id` 字符串或参数 JSON——
   在顺手层文件里 grep `fleet_call` / `JSON.stringify` / `std.json.stringify` 应为零命中，
   只有生成产物里有。

**kill criterion（什么情况下判决作废）**：如果写生成器的过程中发现
`OPERATION_CATALOG` 的 `parameters` 本身不足以确定调用约定——比如某个面的正确用法
需要目录里没有的知识（参数间的互斥、条件必填、`epoch`/`after` 这种要先调另一个面取值
的会话状态）——那么上线层就不是纯函数式可生成的，本判决退回「手写 + 闸门」。
`fleet.events.read` / `fleet.ui.deltas` 的 `epoch`+`after` 看起来正是这种会话式参数：
它们今天在绑定里被整个省略，很可能不是疏忽而是「脚本作者拿不到这两个值」。
**这一条要先查清楚再动手**——它是实施前的第一个任务，不是实施中的意外。

---

## 7. 闸门本身

`tests/fleet_catalog_conformance.rs`，9 条测，committed 状态**全绿**。
已知漂移全部以带注释的 allowlist 钉住（钉的是完整明细，不只是受影响的面名），
注释写明什么情况下该缩、什么情况下该长。

| 测 | 抓什么 |
|----|--------|
| `binding_parsers_see_every_definition_and_every_call` | 解析器自身坏掉（绝不静默匹配到 0） |
| `catalog_length_is_literal_entries_plus_constructor_entries` | 目录出现第三种构造形状，文本解析开始少数 |
| `every_binding_function_names_a_declared_script_surface` | 绑定发明了目录没有的面 |
| `every_binding_function_forwards_the_operation_id_its_surface_declares` | 绑定转发了错的 operation id |
| `every_catalog_surface_is_implemented_by_every_binding` | 新目录条目没有绑定 / 已知缺口被补上 |
| `binding_params_objects_conform_to_the_catalog_parameter_spec` | 参数键名与必填项漂移（含两条绑定一起错的情形） |
| `all_bindings_expose_the_same_surface_map` | 绑定之间互相漂移（N 条通用，不写死两条） |
| `no_binding_declares_the_same_namespace_table_twice` | 命名空间表重复赋值（后一次会静默丢弃前一次的内容） |
| `only_one_script_surface_sits_outside_the_fleet_namespace` | `fleet.*` 命名约定被侵蚀 |

变异验证（改一处 → 跑 → 还原）：改函数名、改 operation id、删一个绑定函数、
一条 body 里放两个 `call(`、单侧加参数、双侧同时加同一个参数——**六种全部转红**。

加第三条绑定的成本：`BINDINGS` 常量加一行，`Flavor` 加一个 `FreeFunction` 臂
（把 `fleet_a_b_c` 映回点分的 `fleet.a.b.c`）。其余八条测自动覆盖它。
