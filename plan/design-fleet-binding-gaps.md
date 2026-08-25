# `fleet.*` 手写绑定的参数缺陷：九个面的判决、修法与不修的理由

| 字段 | 值 |
|------|-----|
| **文档** | `scripts/qjs/lib/fleet.js` / `scripts/lua/lib/fleet.lua` 两条手写绑定与 `src/operations.rs::OPERATION_CATALOG` 之间 9 处参数分歧的**逐条端到端复核**、bug/产品决策二分、以及已落地的修复 |
| 日期 | 2026-08-25 |
| 状态 | 2 处纯 bug 已修（两条绑定同步）；7 处产品决策已定性未派单；闸门 `tests/fleet_catalog_conformance.rs` 已从「钉住现状」升级为「对已修面正面断言一致性」 |
| **产品真理** | [`prd/PRD_02_36_agenterm_qjswasm.md`](../prd/PRD_02_36_agenterm_qjswasm.md)（第三条绑定的归档门锚点） |
| 关联 | [`plan/design-fleet-catalog-binding.md`](design-fleet-catalog-binding.md)（漂移闸门与「生成还是手写」判决）、`src/operations.rs`、`src/client/mod.rs`、`src/script_fleet.rs` |
| 范围声明 | 只改两条绑定 + 闸门 + 本文。**没有**改 `src/**`。所有对宿主侧的判断都是发现，不是改动 |

---

## 1. 一句话

前一轮把 9 个面标成「宿主会拒收」，但那个结论是**读 `validate_fleet_parameters` 源码推出来
的，不是跑出来的**——原文自陈「the broker path needs a live server」。这句话是错的：校验
发生在 CLI 父进程里，早于任何 IPC，**不开服务器就能观测**。本轮把 9 个面逐个真跑了一遍，
结论全部坐实，并在过程中翻出三件更大的事（§5、§6、§7）。

9 个里 **2 个是纯 bug**（签名对、线上键错），已修；**7 个是产品决策**（修了就得改绑定自己的
参数表，现有脚本必须跟着改），不修，逐条写在 §4。

---

## 2. 方法：这次是怎么真跑的

```
cargo build --bin agenterm --features script-qjs
AGENTERM_SCRIPT_BACKEND=qjs agenterm cli script run <fleet.js + try/catch 探针>
```

链路是真的完整一条：`__host.fleet_call` → `src/script_worker.rs` 打包成 broker 的
`"fleet.call"` 帧 → 父进程 `src/client/mod.rs::script_fleet_call` → `operation_by_id`
查目录 → `validate_fleet_parameters`。**校验在 `fleet_mutation_command` 之前、在
`send_ipc_request_to_timeout` 之前**，所以没有服务器完全不妨碍观测拒收；反过来，一个面
如果**通过**了校验，就会走到 IPC 并回 `broker_transport: AgenTerm server is not running`
——这正好是一个免费的阳性对照：能看见 transport 错，就证明参数那关过了。

探针里同时跑了对照组（`fleet.tabs.list`、`fleet.protocol.info`）确认工装本身是活的：
它们回的是 `broker_transport`，不是 `broker_invalid_arguments`。

**为什么用 qjs 不用 lua**：见 §6，lua 那条路上任何一次失败的 `fleet_call` 会直接 abort
掉 worker 进程，脚本连自己造成的拒收都看不见。qjs 把同一个错误变成普通的、可 catch 的 JS
异常，且原样带着 broker 的消息。

lua 侧的载荷则用另一种方式取证：在宿主 lua 调用里覆盖 `__host.fleet_call`，把
`LuaEngine::eval` 自动前置的真实 `fleet.lua` 发出的 JSON 直接打印出来。

---

## 3. 九个面，逐条

下表「实测」列是 broker 原样回的字符串，全部带 `broker_invalid_arguments:` 前缀。

| # | 面 | 发出 | 目录声明 | 实测 | 判决 |
|---|----|------|---------|------|------|
| 1 | `fleet.tabs.set_note(tab_id, note)` | `{"tab_id":…,"note":…}` | `tab`(必填,`stable_tab_id`) + `note`(必填) | `tabs.set-note does not accept parameter tab_id` | **纯 bug，已修** |
| 2 | `fleet.ui.tab.select(id)` | `{"id":…}` | `tab`(选填,`stable_tab_id`) | `ui.tab.select does not accept parameter id` | **纯 bug，已修** |
| 3 | `fleet.terminal.paste(text)` | `{"text":…}` | `NO_PARAMETERS` | `terminal.paste does not accept parameter text` | 产品决策 |
| 4 | `fleet.ui.composer.send(text)` | `{"text":…}` | 只有 `tab`(选填) | `ui.composer.send does not accept parameter text` | 产品决策 |
| 5 | `fleet.ui.input.wheel(delta)` | `{"delta":3}` | `x`/`y`/`delta_y` 必填 + `units`/`mods` 选填 | `ui.input.wheel does not accept parameter delta` | 产品决策（且 §5 表明修了也不通） |
| 6 | `fleet.ui.hello()` | `{}` | `minimum`/`maximum` 必填 + `client_id` 选填 | `ui.hello requires parameter minimum` | 产品决策 |
| 7 | `fleet.ui.deltas()` | `{}` | `epoch`/`after` 必填 + `limit` 选填 | `ui.deltas requires parameter epoch` | 产品决策 |
| 8 | `fleet.events.read()` | `{}` | `epoch`/`after` 必填 + `limit` 选填 | `events.read requires parameter epoch` | 产品决策 |
| 9 | `fleet.events.wait(timeout_ms)` | `{"timeout_ms":100}` | `epoch`/`after`/`kind` 必填 + `tab`/`timeout_ms` 选填 | `events.wait requires parameter epoch` | 产品决策 |

**前一轮的结论 9/9 坐实**，没有一个是误判。这点值得说清楚：任务要求「如果某条其实没被拒，
说出来」——没有这样的条目。前一轮错的不是结论，是**结论的成色**（自陈未验证，而且给出的
不能验证的理由是假的）。

### 3.1 两处纯 bug 的修法

判据是任务给的：修完之后**脚本作者看不见任何变化**，JS/Lua 函数签名一字不动。

```js
// scripts/qjs/lib/fleet.js
fleet.tabs.set_note = function (tabId, note) {
  return call("tabs.set-note", JSON.stringify({ tab: tabId, note: note }));  // was tab_id:
};
fleet.ui.tab.select = function (id) {
  return call("ui.tab.select", JSON.stringify({ tab: id }));                 // was id:
};
```

```lua
-- scripts/lua/lib/fleet.lua
function fleet.tabs.set_note(tab_id, note)
    return call("tabs.set-note", std.json.stringify({ tab = tab_id, note = note }))
end
function fleet.ui.tab.select(id)
    return call("ui.tab.select", std.json.stringify({ tab = id }))
end
```

两条同步改，因为这两个文件是**逐行对译**的：同一段脚本逻辑不论哪个引擎跑，都必须产生同一
个 Fleet 操作。只改一边就是悄悄毁掉这条保证，而 `all_bindings_expose_the_same_surface_map`
是发现它的唯一岗哨。

第 1 条有个独立的旁证：**rh 那条绑定（`src/script_fleet.rs`，用 Rust 写的第三条绑定）一直
发的就是 `"tab"`**：

```rust
engine.register_fn("set_note", |service: &mut FleetTabs, tab_id: &str, note: &str| {
    service.0.mutate("tabs.set-note", json!({ "tab": tab_id, "note": note }))
});
```

签名同样是 `(tab_id, note)`。也就是说「参数名叫 tab_id、线上键叫 tab」这件事在这个仓库里
早就有正确答案，手写绑定只是没抄对。这也顺带否掉了前一份文档里那句辩护：那颗语法糖不但可以
机械推导，它本身就是 bug 的成因。

修后实测：

```
tabs.set-note {"note":"hello","tab":"@1"}  → broker_transport（过了校验，走到 IPC）
ui.tab.select {"tab":"@2"}                 → broker_operation_unknown: no Fleet adapter
                                              exists for ui.tab.select（过了校验，见 §5）
```

lua 侧捕获到的载荷与 js 侧逐字节相同。

---

## 4. 七个产品决策，具体是什么

「产品决策」不是「以后再说」的委婉语。每一条都是一个**必须有人拍板的、会打破现有脚本的
签名变更**，下面把选项写死，方便直接拍。

### 4.1 `fleet.terminal.paste(text)` — 这个 `text` 参数从来就不存在

`terminal.paste` 的 `OperationSpec` 是 `NO_PARAMETERS`，而它的适配器是
`ui-action terminal-paste`——**粘贴的是剪贴板**，宿主根本没有接收文本的入口。rh 绑定同样
是零参的 `paste()`。所以目录没错，错的是 JS/Lua 凭空发明了一个参数。

- 选项 A（推荐）：签名改成 `fleet.terminal.paste()`。实测 `{}` 能过校验并走到 IPC。传 text 的调用方必须改成先设剪贴板。
- 选项 B：给目录加一个 `text` 参数并给宿主加真实通路。这是新功能，不是修 bug。
- **不能**两全：保留 `text` 参数就等于保留一个 100% 被拒的函数。

### 4.2 `fleet.ui.composer.send(text)` — 同样是凭空发明的 `text`

`ui.composer.send` 声明的参数只有 `TAB_TARGET_PARAMETERS` 里那个选填 `tab`。语义是
「把 tab X 的输入框里已经有的内容发出去」，不是「发送这段文本」。

- 选项 A：签名改成 `fleet.ui.composer.send(tab)`——语义完全变了，`send("hi")` 这种调用不是改参数就能救的，得先 `ui.input.key` 把字打进去。
- 选项 B：目录扩参。同 4.1 选项 B，是新功能。
- 注意：这个面**另外**还卡在 §5（没有 mutation adapter），即使参数对了也回 `broker_operation_unknown`。

### 4.3 `fleet.ui.input.wheel(delta)` — 缺三个必填，而且修了也不通

声明是 `x`/`y`/`delta_y` 三个必填。签名要变成 `wheel(x, y, delta_y)`。

但**先别派这个单**：`x`/`y`/`delta_y` 的 `value_type` 是 `"number"`，而
`validate_fleet_parameters` 的 `match spec.value_type` 根本没有 `"number"` 这条臂，落到
`_ => false`。实测三种形状全拒：

```
ui.input.wheel   {"x":1,"y":2,"delta_y":3}  → parameter x must be number
ui.input.pointer {"x":1,"y":2}              → parameter x must be number
ui.input.pointer {"x":1.5,"y":2.5}          → parameter x must be number
ui.input.pointer {"x":"1","y":"2"}          → parameter x must be number
```

先修宿主（给 `validate_fleet_parameters` 加 `"number" => value.as_f64().is_some()`），
再谈签名。闸门里
`every_catalog_value_type_is_one_the_broker_validator_can_accept` 现在钉住了这个洞，
宿主一修它就红，提醒回来重新分类。

### 4.4 `fleet.ui.hello()` — 缺两个必填

声明 `minimum`/`maximum`（`uint32`，必填）+ `client_id`（选填）。签名要变成
`hello(minimum, maximum)`，一个原本零参的「ping」变成带协议版本协商的握手。
另见 §5：`ui.hello` 连 broker 分支都没有。

### 4.5 `fleet.ui.deltas()` — 缺两个必填

声明 `epoch`(必填) / `after`(必填) / `limit`(选填)。签名要变成 `deltas(epoch, after)`。
调用方还得先从别处拿到 `epoch`——这是把一个「给我增量」的糖，换成一个必须自己管游标的
真 API。另见 §5：`ui.deltas` 同样没有 broker 分支。

### 4.6 `fleet.events.read()` — 缺两个必填，外加一个目录与校验器互相矛盾的坑

声明 `epoch`(必填) / `after`(必填) / `limit`(选填)。签名要变成 `read(epoch, after, limit?)`。

**坑**：`limit` 目录里写的是选填，但 `validate_fleet_parameters` 末尾另有一段预算检查：

```rust
if operation.id == "events.read"
    && parameters["limit"].as_u64().is_none_or(|v| v == 0 || v > budgets.event_items as u64)
```

`is_none_or` 意味着**缺 `limit` 直接判死**。实测：

```
events.read {"epoch":"e","after":0}          → events.read limit exceeds the invocation event budget
events.read {"epoch":"e","after":0,"limit":8} → broker_transport（过了）
```

所以 `limit` 事实上是必填，目录的 `required: false` 是假的。rh 绑定给了默认 `limit: 64`，
新签名应照抄这个默认。这是**目录侧的发现，不在本轮改动范围**。

### 4.7 `fleet.events.wait(timeout_ms)` — 缺三个必填，同样的选填-其实必填坑

声明 `epoch`/`after`/`kind` 必填 + `tab`/`timeout_ms` 选填。签名要变成
`wait(epoch, after, kind, timeout_ms)`（rh 绑定另有一个带 `tab` 的重载）。

同样的坑：`timeout_ms` 标选填，但预算检查 `is_none_or` 让它事实必填。实测：

```
events.wait {"epoch":"e","after":0,"kind":"k"}                  → timeout_ms exceeds the invocation wait budget
events.wait {"epoch":"e","after":0,"kind":"k","timeout_ms":10}  → broker_transport（过了）
```

---

## 5. 比这 9 条更大的事：宿主真正接得住的面只有 17 个

修 `fleet.ui.tab.select` 的过程里冒出来的：参数对了之后，回的是

```
broker_operation_unknown: no Fleet adapter exists for ui.tab.select
```

`src/client/mod.rs::fleet_mutation_command` 是一张写死的 `match`，只认 **9** 个 mutation：
`ui.tabs.show` / `ui.tabs.hide` / `ui.tabs.toggle` / `ui.window.activate` /
`terminal.paste` / `ui.tabs.set-width` / `tabs.set-note` / `server.kill` /
`workspace.shutdown`。

`handle_script_broker` 的 observe 分支只认 **8** 个：`protocol.info` / `workspace.info` /
`ui.snapshot` / `tabs.list` / `tabs.active` / `pane.capture` / `events.read` /
`events.wait`。

**77 条目录里，脚本侧真正可达的是 17 条。** 已经绑定的 29 个面里，至少
`fleet.ui.tab.select`、`fleet.ui.tab.new_child`、`fleet.ui.composer.send`、
`fleet.ui.tree.toggle`、`fleet.ui.bootstrap`、`fleet.ui.hello`、`fleet.ui.deltas`、
`fleet.control_center.*` 这一批即使参数完全合规也只会回 `broker_operation_unknown`。

这直接改变了「29 个已绑定面里 9 个坏」这句话的分量：**参数合规不等于能用**。最锋利的例子是
`fleet.ui.input.pointer`——它发的 `{x, y, action}` 三个键目录全都声明了，漂移为零，闸门判它
合规，而宿主对它的每一次调用都拒（§4.3 的 `"number"` 洞）。闸门现在用
`every_catalog_value_type_is_one_the_broker_validator_can_accept` 把这件事写死，并在注释里
点明「合规」与「能用」是两个属性。

---

## 6. lua 那条路上，失败的 `fleet_call` 会 abort 掉整个 worker

用 lua 后端跑同一个探针时，进程直接死：

```
thread '<unnamed>' panicked at core/src/panicking.rs:225:5:
panic in a function that cannot unwind
   5: mlua_sys::lua51::lua::lua_error
   6: mlua::state::util::callback_error_ext::<…create_callback::call_callback…>
  10: <mlua::function::Function>::call::<mlua::value::Value, ()>
thread caused non-unwinding panic. aborting.
{"code":"host_worker_crash","exit_class":"host","message":"script worker … exited before a valid result (None)"}
```

`crates/agenterm-lua/src/lib.rs` 里注入的 `fleet_call` 在宿主回错时返回
`Err(mlua::Error::runtime(...))`，mlua 用 LuaJIT 的 `lua_error` 抛出，longjmp 穿过一个不能
unwind 的 Rust 帧。**任何**失败的 `fleet_call` 都会这样——包括本文列的 9 个拒收，也包括
「服务器没开」这种日常错误。后果是 lua 脚本连 `pcall` 都救不回来：不是拿到错误，是整个
worker 没了。

这是宿主缺陷，与参数一致性无关，不在本轮改动范围。qjs 那边同一个错误是普通 JS 异常
（`crates/agenterm-qjs/src/host.rs` 用 `Exception::throw_message`），可以 catch，消息原样。
**两条绑定「逐行对译」的等价性到错误处理这一层就断了**，这一点前面没人记过。

---

## 7. 47 个一个绑定都没有的面

`OPERATION_CATALOG` 77 条（44 条长写 + 33 条由 `nullary_ui_action()` 构造），其中 76 条在
`fleet.*` 命名空间（第 77 条是唯一的例外 `FleetTerminal.capture` → `pane.capture`）。两条
手写绑定各实现 **29** 个，**47 个两边都没有，而且缺的是同一批 47 个**——这正是
copy-and-compare 买到的（互相一致）和付出的（一起偏离声明方）。

这 47 个是**功能缺口，不是 bug**，本轮明确不做。清单（`tests/fleet_catalog_conformance.rs::unimplemented_surfaces`
是权威副本）：

`fleet.terminal.copy_selection`、`fleet.terminal.mouse`、`fleet.ui.cwd_editor.open`、
`fleet.ui.cwd_editor.prepare`、`fleet.ui.cwd_editor.prepare_append`、
`fleet.ui.cwd_editor.prepare_replace`、`fleet.ui.cwd_editor.send_now`、
`fleet.ui.font.decrease`、`fleet.ui.font.increase`、`fleet.ui.input.key`、
`fleet.ui.instance_picker.{cancel,confirm,next,open,prev,select}`、
`fleet.ui.locale.toggle`、`fleet.ui.modal.{cancel,confirm}`、
`fleet.ui.new_terminal.open`、`fleet.ui.server_strip.select`、
`fleet.ui.settings.apply`、`fleet.ui.settings.inherit.{font,size,theme}`、
`fleet.ui.settings.open`、
`fleet.ui.settings.preset.{classic_day,classic_night,fancy_day,fancy_night}`、
`fleet.ui.settings.reset_overrides`、`fleet.ui.settings.scope.{current,defaults}`、
`fleet.ui.settings.theme.{dark,light}`、`fleet.ui.tab.close`、`fleet.ui.tab.edit`、
`fleet.ui.tab.editor.{cancel,save}`、`fleet.ui.tab.new`、
`fleet.ui.window.{close,maximize,minimize,resize,restore}`、
`fleet.ui.window_close.keep_server_running`、
`fleet.ui.window_close.stop_server_and_exit`。

33 个是 `nullary_ui_action()` 构造的零参 UI 动作，14 个是长写条目。**补之前先读 §5**：
这 47 个里绝大多数在宿主侧同样没有 adapter，补绑定只会得到一批回 `broker_operation_unknown`
的函数。「补齐 47 个」的正确形态是「先补 `fleet_mutation_command`，再补绑定」。

---

## 8. 闸门变成了什么

`tests/fleet_catalog_conformance.rs` 从 9 测涨到 12 测，性质也变了一半：

| 测试 | 性质 | 说的是 |
|------|------|--------|
| `fixed_bindings_send_exactly_the_declared_parameter_names` | **新增，正面断言** | 两个已修面必须**恰好**发目录声明的那组键。用「相等」不用「不在漂移表里」：漂移表忽略缺失的选填参数，如果有人把 `note` 删了它不会响 |
| `remaining_parameter_drift_is_documented_as_product_decisions` | 新增，文档防腐 | 漂移表里剩下的每一条，本文都得点名；已修的两条也得留痕 |
| `every_catalog_value_type_is_one_the_broker_validator_can_accept` | 新增，钉住 §4.3 | `"number"` 是目录声明而校验器无法满足的唯一类型，波及 `fleet.ui.input.pointer` / `fleet.ui.input.wheel` |
| `binding_params_objects_conform_to_the_catalog_parameter_spec` | 收紧 | 期望表从 9 条降到 7 条 |
| 其余 8 测 | 不变 | 解析自证、目录计数、正反两向、绑定互等、命名空间卫生、命名空间例外 |

模块头注释也改了：原来那句「the broker path needs a live server」是错的，现在写的是真实的
复核方法，免得下一个人又因为同一个假理由放弃验证。
