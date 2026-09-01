# agenterm-qjswasm

AgenTerm 自己的脚本引擎。`.qjs` 用**纯 Rust** 编译成 `.wasm`，`.wasm` 直接跑；
核是 [tinyvm](https://github.com/partnernetsoftware/tinyvm)——无 JIT、装载期校验、
上限在核。

**不是** `rquickjs`，**不链** QuickJS C 库，**不是**（目前还不是）一个 JavaScript 引擎。

产品真理：[`prd/PRD_02_36_agenterm_qjswasm.md`](../../prd/PRD_02_36_agenterm_qjswasm.md)。
实现设计：[`plan/design-agenterm-qjswasm.md`](../../plan/design-agenterm-qjswasm.md)。

## 管线

```text
.qjs 源码
   │  ① 词法 / 语法 / 降级      tinyvm-qjs，纯 Rust，ECMA-262 为语义权威
   ▼
标准 .wasm 字节
   │  ② decode / validate / Limits      tinyvm
   ▼
解释执行，不生成机器码
```

编译器（①）2026-08-24 从本 crate 迁往上游 `tinyvm-qjs`：它一行 agenterm 概念都没有，
按「通用引擎能力归 tinyvm、业务归 agenterm」这条分层线，它属于上游。本 crate 留下的是
真正的业务——`agenterm.*` 门、槽、预算策略、接线。`CompileError` 原样再导出；
`compile_qjs` 是本 crate 的函数，签名不变，但它现在**带着门的声明表**进上游编译器
（`compile_qjs_m1_with`），因为「有哪些宿主能力」正是业务那一半。撤销记录见
[PRD 36](../../prd/PRD_02_36_agenterm_qjswasm.md) 与
[设计稿 §2](../../plan/design-agenterm-qjswasm.md)。

`.wasm` 输入跳过 ①，从 ② 进。**两种输入在核这一层完全同待遇**——这就是「一个引擎跑
两种东西」的确切含义，不是两条管线共用一个名字。

## `.qjs` 能跑什么（诚实边界，2026-08-25 在 rev `14a641a` 上整表复测）

**这张表是跑出来的。** 能跑的每一条都编译并执行过，拿到的就是这里写的值；拒绝的每一条
都编译过，拿到的就是这里引的诊断。读上游源码、或转述别人的报告，都不算证据——这两种口径
在本仓各骗过一次人：`%` 与 `typeof` 早已支持却还挂在拒绝表上，以及本文件曾说这个 crate
在工作区外而它在里面。

本轮复测走的是**产品自己的路**（`AGENTERM_SCRIPT_BACKEND=qjswasm agenterm cli script
run FILE`），不是 crate 内的测试夹具——同一条源码经过编译器、装载闸门、槽、门、
completion value 投影全程，任何一段掉链子这里都看得见。`e1122ff → 14a641a` 这一跳带来
的东西多，整表逐行重跑过。

**能跑：**

- **数字是 ECMA-262 binary64**，不是 i32：`1/0` 是 `Infinity`、`0/0` 是 `NaN`、
  `2147483647 + 1` 是 `2147483648` 不回绕，`3 / 2` 是 `1.5`。
  **小数字面量也能写了**（`05d506e`）：`1.5` / `.5` / `1.` / `1e3` / `2E2` / `1.5e-3`
  全读，`0.1 + 0.2` 是 `0.30000000000000004`——这句话才说明它们是 double 不是被迁就的十进制。
  整数也不再卡在 `i32`：`2147483648` 可以写，`9007199254740993` 读回
  `9007199254740992`（ECMA-262 说的最近 double，每个引擎都一样）。
  **上一版说「字面量还不能写小数」，已作废。**
  仍各自撞诊断的是**别的进制**：`0x10`、`0o17`、`0b101`、`1_000`。
- **其他值**：字符串（转义与 `\u{…}` 已解码）、`true` / `false`、`null`、`undefined`。
- **对象**：字面量 `{a: 1}`、点取属性、`o["a"]` 索引、属性赋值、任意嵌套（`o.a.b`）。
- **数组**（`c0d7ae4` 到）：字面量 `[1, 2, 3]`、`a[i]` 读写、`a.length`、任意嵌套、
  与对象互相嵌套。越界读是 `undefined` 不是 fault；越界写把中间补成 `undefined`
  而不是 hole（这个引擎没有 `in` / `forEach`，两者不可分辨——见上游 `arr_set`）；
  `typeof []` 是 `"object"`，`[]` 是真值，`===` 是引用相等。**上一版说「数组还没有」，
  已作废。**
  三条边界照实说：**字符串 key 不是索引**（`a["0"]` 是 `undefined`，而 ECMA-262
  10.4.2.1 读元素 0——具名分歧）；**非索引属性写是有名字的拒绝**（`a.foo = 1` → `InvalidWrite("an Array key that is not an index below 16777216")`，tinyvm 16da41d 之前是无名 trap；密集向量里
  没地方放，丢掉比 trap 更糟）。**上一版说「没有任何数组方法」，已作废**——见下条。
- **语句**：`let` / `const` / `var`（真作用域 + 文本可判定的 TDZ）、块、`if`/`else`、
  `while`、三段式 `for`、`return`、`throw`、`try`/`catch`/`finally`，以及脚本的
  ECMA-262 completion value（`1 + 2;` → `3`）。
- **函数**：声明式带参数、递归与互递归、嵌套声明、读模块顶层绑定。
  **函数是值**：`let f = function(a){...}; f(1)` 可以，`return function(){...}` 再调用
  也可以。**捕获外层局部变量的闭包也到了**（`eb9229c`）：捕获按**绑定**不按值——
  `let a = 1; function i(){return a;} a = 2; i()` 是 `2` 不是 `1`，闭包里写回声明处也看得见。
  参数一样算绑定（`function mk(n){ return function(){ return n; }; }` 可用），
  任意嵌套深度可用，同一个函数表达式的两个实例各有各的环境。
  **上一版说「仍然拒绝」，已作废。**
- **运算符**：赋值与复合赋值、`||`、`&&`、`==`/`!=`/`===`/`!==`、`<` `<=` `>` `>=`、
  `+` `-`、`*` `/`、`%`、`typeof`、前后缀 `++`/`--`、一元 `+ - !`、括号、**`?:`**。
- **三个 ECMA-262 转换都到了**（`14a641a`）：`"n=" + 1` 是 `"n=1"`、`"2" * 2` 是 `4`、
  `"a" < "b"` 是 `true`、`1 == "1"` 是 `true`。**上一版 README 说这些 trap，已作废。**
- **`JSON` 是一个真名字**：`JSON.stringify({a:{b:"c"}})` 给 `{"a":{"b":"c"}}`，
  `JSON.parse("{\"a\":3}").a` 给 `3`。**上一版说「`JSON` 今天不是名字」，已作废。**
  **含数组的 JSON 现在也行**（`c0d7ae4`）：`JSON.parse("[1,2,3]")[1]` 是 `2`，
  `JSON.stringify([1,[2,{c:3}]])` 往返一致，`[{"id":"tab1"}]`——也就是 `tabs.list`
  的真实形状——解析出来能索引。两条规范细节容易记反：`[undefined,1]` 是 `[null,1]`
  而 `{a:undefined,b:1}` 是 `{"b":1}`（25.5.2.5 第 8 步 vs 25.5.2.4 第 5 步，数组的
  下标是位置性的，丢一个会把后面全部改号）；自引用数组和自引用对象一样是可 catch 的
  TypeError。**上一版说「含数组的 JSON 仍然不行」，已作废。**
- **模板字面量**（`3d8ed07` 到）：`` `abc` ``、`` `a${x}b` ``、任意嵌套
  （`` `a${`b${c}`}d` ``）、替换里可以写任何表达式包括带花括号的
  （`` `${ {a:7}.a }` ``）。替换取的是 `ToString`，所以 `` `${1}${2}` `` 是 `"12"`
  而不是 `3`。模板文本里可以直接换行（字符串不行），且 TV 把 `\r\n` 与单个 `\r`
  都归一成一个 `\n`——同一段模板在 CRLF 文件和 LF 文件里意思一样。
  **上一版把它列在「拒绝」里，已作废。**
  实现上它折成 `+` 链（13.2.8.6 与 13.15.3 是同一个算法），所以**没有模板的程序一个
  字节都不多付**——实测六种程序 Δ 全 0，且模板与它等价的拼接编译出逐字节相同的模块。
  仍拒绝的是**带标签**的模板（`` t`a` ``）：那是一次调用，第一个实参得是带 `raw`
  的冻结 cooked 数组——`raw` 是个属性，而这引擎没有属性定义，做不出那个形状。
- **箭头函数**（`ff5d2ac` 到）：`(x) => x + 1`、单参数免括号 `x => x`、空参数表
  `() => 7`、简洁体与块体都行、任意柯里化（`(x) => (y) => x + y`）、捕获照常
  （`function mk(n) { return () => n; }`）。**上一版把它列在「拒绝」里，已作废。**
  实现上**箭头在这个引擎里就是函数表达式**——15.3 用来分开两者的四条（`this`、
  `arguments`、`new`、函数属性）这个引擎一条都没有——所以两种写法编译出逐字节
  相同的模块，无箭头程序也一个字节不多付。**这个等价是有条件的**：`this` 哪天落地
  它就作废，上游有测试钉着那四个「没有」，那天会响。
  仍拒绝的是**参数表**里超出一个普通名字的东西：默认参数 `(a = 1) => a`、rest
  `(...a) => a`、解构 `([a]) => a`——跟普通函数的参数表是同一件事，一起排期。
- **方法**（`130e929` 到）：字符串 `trim` / `indexOf`，数组 `push` / `pop` / `map`。
  `map` 的回调可以是箭头、具名函数，能捕获外层绑定，`map` 可链。
  `trim` 认的是**整个** ECMA-262 12.2 WhiteSpace + 12.3 LineTerminator
  （含 `Zs` 全部，`"\u{3000}ab\u{2003}".trim()` 是 `"ab"`）；`indexOf` 的位置是
  **UTF-16 码元**，与 `.length` 对得上。普通对象上同名属性不受影响
  （`{trim: f}.trim()` 调的还是 `f`）。
  **上一版说「没有任何数组方法」「`push`/`map` 读出来是 undefined」，已作废。**

  **这五个方法的绑定机制是量出来的，不是选出来的**：上游
  `research/method-binding/` 实现了三种把接收者送到方法体的做法
  （`this` 走调用约定 / 属性读时装进闭包 / 调用点特化），用一份**在任何实现之前
  写好**的语料把三种都跑过，再按边际成本比。调用点特化胜出，
  另外两种连同它们的 feature 一起删掉了。判决 trace 与数字在那边的 `RESULTS.md`。

  仍然没有的：其它字符串方法（`trim` / `trimStart` 之类）与数字方法
  （`(1).toPrecision`）——**读就 trap**；数组上没落地的方法（`filter` / `splice`）
  **读出来是 `undefined`，调用才 trap**。两种接收者规矩不同是上游刻意保留的，
  理由见「内建属性」那条。
- **`agenterm.*` 门**：`print` / `fleet_call` / `fleet_result`——见下一节。

**明确拒绝**（除注明外全在编译期）。分三类，因为拒绝的**理由**不是一个：

1. **语法认得，能力还没有**——诊断形如「this engine does not support X yet」：
   数组 elision（`[1, , 2]`——hole 不是 `undefined`，引擎没法分辨，所以按名字拒绝
   而不是二选一）、`class`、`switch`、`break`/`continue`、`for…of` / `for…in`、`do`/`while`、
   带标签的模板（`` t`a` ``——**普通模板已经不在这张表上了**，见上）、
   默认 / rest / 解构参数（**箭头函数本身也不在这张表上了**，见上）、
   位运算与移位、`**`、`??`、可选链、逗号运算符、BigInt、
   `new` / `delete` / `void` / `in` / `instanceof`、展开与 rest、解构、默认参数、
   `async`/`await`、`import`、带标签的语句。（**捕获闭包已从这张表离开**，见上。）
2. **根本没有全局对象。** `Math`、`String`、`Number`、`Object` 今天不是名字，写它们撞的是
   门的诊断：``this engine has no host function named `Math`; this embedder declares
   `print`, `fleet_call` and `fleet_result` ``。`JSON` 是唯一的例外——它是真的实现了。
   内建属性只有**一个**（`4f6af7c`）：`"ab".length` 现在给正确答案，
   且数的是 **UTF-16 码元**不是 UTF-8 字节——`"café".length` 是 4，`"😀".length` 是 2。
   **上一版说它是运行期 trap，已作废。** 但那是 `obj_get` 里的**一条臂**，不是原型链：
   字符串的**其它**属性仍然 trap（`"ab".trim` / `"ab".trimStart`），
   而且是**故意**不返回 `undefined`——那两个在真 JS 里是函数，`undefined` 会是
   看起来像对的错答案。数字与布尔的属性也仍然 trap（`(1).toPrecision`）。
   顺带：这条臂是门控的，所以**没写 `.length` 的程序比上一版更小**（`return 1;`
   少 19 字节）——`__len` 之前在无条件 runtime 里，发射了但没人调。
3. **解析就没过**——正则字面量 `/a/`（「needs an operand here, and found a `/`」）、
   生成器 `function*`（「needs a name for the function declared here」）。

**诊断的诚实度**：上一版记的两条缺口修掉了一条。带标签的语句现在正确报
「does not support labelled statements yet」（曾经错报成三目运算符）。仍然错的一条：
**`for (const x of y)`** 报的是「needs a value for the `const` binding `x`」而不是 `of`
（同一句写成 `let` 或 `var` 就正确报 `of`）。

**运行期缺口，2026-08-25 复核后的准确说法**（上一版这里写「本层两条」，**两条都记错了，
下面是订正**）：

- **未捕获的 `throw` 曾经报成裸 trap——已修。** 现在报
  `the script threw a value and nothing caught it`，`QjswasmError::UncaughtThrow`
  自成一类，不再是 `Trap`。做法就是上游一直准备好的那条：`explain()` 读
  `tinyvm_qjs::guest_fault()`，把堆耗尽与未捕获抛出**一起**从裸 trap 里分出来。
  锁在 `tests/qjs_guest.rs` 两条：一条钉分类，一条钉「上一次调用写下的 fault word
  不许污染下一次」。**还差的是抛出的值本身**——编译后的模块不导出持有它的 global，
  上游 `GuestFault::UncaughtThrow` 明说这是宿主边界的决定而不是抛出的问题；要文本就
  自己 `catch`。
- **「对象不能当 completion value 出来」这条记错了两处，已作废。**
  一，**归属错**：`Value` 没有 Object 变体的是**上游** `tinyvm-qjs`（`repr::host_decode`
  的 `TAG_OBJECT` 臂直接 `Err`），不是本 crate 的脸；本层只是把上游的话原样转述。
  二，**因果错**：它根本不挡归档门。`fleet.qjs` 曾以 `fleet;` 结尾，那是**本仓自己加的**
  一行——`fleet.js` 没有这一行，因为它是**库**不是程序。拿 `script run` 去跑一个库文件
  是范畴错误，跟对象能不能出来无关（去掉 `fleet;` 之后倒在最后一句赋值求值出来的
  **函数**上，同样出不来，也同样不重要）。库的正确检查是 `script check`，答 `OK`；
  库的正确用法是被脚本 `use`，这条现在有测试（见下）。

  这条错得值得记下来：它把一个上游的表示层限制，误报成了本层的产品级拦路石，
  而真正挡着门的是**绑定只港了 29 个操作里的 8 个**——一件谁都能数出来、却没人去数的事。

### `.qjs` 已经够得着门（2026-08-25，rev `1271a00` 落地，`e1122ff` 上复测）

曾经这里写的是「自由名字一律在编译期被拒，所以 `print` / `fleet_call` 只有手写 `.wasm`
客人能调」——**那一条已作废**。脚本直接写三个名字：

```js
print("hello");
let status = fleet_call("tabs.list", "{}");   // 0=Ok 1=Err 2=NoBridge
if (status === 0) { return fleet_result(); }
return status;
```

这段是**原样跑过的**：`stdout` 是 `"hello"`，桥收到 `("tabs.list", "{}")`，返回值是桥的
答案；同一段不给桥再跑一遍，返回 `2`，而 `"hello"` 照样进 `stdout`。

门**一个字没改**：客人仍然导入那四个原始 i32 函数（下面「宿主门 ABI」那张表），与手写
`.wasm` 客人同一张 import 表。拆包是**编译器**的活——JS 字符串拆成 `(ptr, len)`，两趟取回
的字节（`fleet_result_len` → bump 分配 → `fleet_result`）组装回 JS 字符串。门不认识 JS
值，这个方向就是设计本身：让门说 V1 双字会弄坏每一个手写客人，也会把一门语言的值表示
泄进一个本该服务任意客人的边界（`plan/design-agenterm-qjswasm.md` §6.5）。

声明表在 `src/host.rs::declarations()`，公开面是 `door_declarations()`。
**脚本可见名 = field 名**，不改名：读上面那张门表写下来的，就是能用的名字。

动手之前值得知道的，每条都是实测：

| 事实 | 实测 |
|------|------|
| 三条声明发射四个 import——`fleet_result` 是两趟 | `print("x"); fleet_call("o","p"); return fleet_result();` → `agenterm.print` · `fleet_call` · `fleet_result_len` · `fleet_result` |
| `fleet_result_len` **不是脚本能写的名字**，长度那趟归编译器 | 写它 → ``this engine has no host function named `fleet_result_len` `` |
| 只有脚本**语法上提到**的声明才变成 import | `return 1 + 1;` → 零个 import。按「提到」不按「可达」：`if (false) { print("x"); }` 仍然 emit `agenterm.print` |
| 参数**类型**是编译期查的，门只收字符串 | `print(42)` → ``cannot pass a Number to argument 1 of the host function `print`, which is declared to take a String`` |
| 参数**个数**也是编译期查的 | `print("a","b")` → ``given the host function `print` with 1 argument(s), and this call passes 2`` |
| 传进去的字符串不必是字面量 | `let op = "tabs" + ".list"; fleet_call(op, "{}")` → 桥收到 `"tabs.list"` |
| `print(...)` 求值为 `undefined` | `typeof print("x")` → `"undefined"` |
| 没调 `fleet_call` 就 `fleet_result()` → 空串，不是错 | `"[" + fleet_result() + "]"` → `"[]"` |
| 没有桥 → status `2`，脚本继续跑，不 trap | `return fleet_call("a","b")` → `2` |
| 门名字**不是保留字**，脚本自己的绑定盖得住 | `function print(m) { return 1; } return print("x");` → `1`，宿主门没被调到；声明只是自由名字的兜底 |

**上一版这里写「status 是数字，而 Number 的 ToString 还没实现，所以 `"status:" + s`
会 trap」——已作废。** `14a641a` 之后 `"status=" + fleet_call(...)` 实测打印
`status=1`。同一份 README 上面已经写了三个转换都到了，这段是漏改的旧话。

证据在 `tests/qjs_door.rs`（13 条：状态 0/1/2 三条路、`print` 进 `Outcome::stdout`、
`print` 求值为 `undefined`、两个宿主侧上限、门外的名字被拒且诊断把声明了哪些列出来、
以及把 emit 出来的 import 表解码出来逐字对 `src/host.rs::SIGNATURES`）。

想要一份**够不着门**的产物（import 表按构造为空）用 `compile_qjs_without_door`——实测它
对 `print("x")` 报 ``this engine finds no declaration of `print` ``，对 `return 1 + 1;`
发射零个 import。`check` 与 `execute` 都走 `compile_qjs`，两边看见的是同一门语言。

### 第二扇门：`tool.*`（只给工具脚本，沙箱永远开不了）

PRD 36「A1.1 的答案」定的：`.qjs` 有两种。**沙箱 `.qjs`** 只看见 `agenterm.*`；
**工具 `.qjs`**（CI 门、构建、qualification）多一扇 `tool.*`——文件系统、子进程、环境变量。
区别在**谁能开**，不在脚本写了什么：

- `compile_qjs` **不知道这扇门存在**：写 `fs_exists("/")` 撞的是能力诊断，列出来的也只有
  `print` / `fleet_call` / `fleet_result`。开门的编译入口是 `compile_qjs_tool`
  （及 `compile_qjs_tool_with_modules` / `check_qjs_tool_with`）。
- `Engine::new()` / `with_budget` 的槽**装载期就拒** `tool.*` import，诊断点名 import 和
  `Engine::with_tool_door`——同一份字节换到 `with_tool_door` 的引擎就绑上真门。
  `validate_wasm` 与 `validate_wasm_tool_with` 分别与两种引擎口径一致。
- **零成本**：没提到 `tool_*` 名字的脚本，两个入口编出的 `.wasm` **逐字节相同**
  （实测 `return 1;` 9765 字节，改动前后 sha256 一致；`tests/tool_door.rs` 锁四个程序）。

门表（`src/tool.rs`，公开面 `tool_door_declarations()`；脚本可见名 = field 名的 `.` 换 `_`）：

| 脚本写 | wasm import | 答什么 |
|--------|-------------|--------|
| `fs_exists(p)` / `env_has(n)` | `tool.fs.exists` / `tool.env.has` | 直接 `1`/`0`；`-1` = 问不了（非 UTF-8），诊断暂存 |
| `fs_read_to_string(p)` `fs_write(p, text)` `fs_create_dir_all(p)` `fs_remove_file(p)` | `tool.fs.*` | status `0`/`1`；文本或诊断暂存 |
| `fs_metadata(p)` / `fs_read_dir(p)` | `tool.fs.metadata` / `tool.fs.read_dir` | status；暂存 JSON `{is_file,is_dir,len}` / 按名排序的 `[{name,path,is_file,is_dir,is_symlink}]` |
| `fs_tree_summary(p,max_entries)` | `tool.fs.tree_summary` | status；原生侧在显式 entry 上限内递归统计文件数、逻辑字节、mtime 与首层 bucket，固定大小 JSON 过桥；超限整体拒绝，不返回截断真相 |
| `process_command(spec_json)` | `tool.process.command` | status；spec `{program,args,current_dir,env,timeout_ms,stdin_text}`（未知字段拒），暂存 `{exit_code,success,stdout,stderr,timed_out}`；无 `timeout_ms` 默认 60 s 后杀 |
| `process_id()` | `tool.process.id` | 直接 pid |
| `env_get(n)` / `env_cwd()` | `tool.env.get` / `tool.env.cwd` | status；值或诊断暂存（未设置是 status `1`，不是空串——要空串用 `env_has`） |
| `tool_result()` | `tool.result_len` + `tool.result` | 与 `fleet_result` 同一套两趟取回；**独立**于 fleet 的暂存区，互不覆盖 |

预算与审计走 fleet 那一套：暂存答案受 `max_bridge_result_bytes`（超了是拒绝不是前缀），
`process.command` 抓的 stdout/stderr 同一个数；操作里 panic 报 `QjswasmError::Door`
不伪装成 status 1。**每次调用都记名**：`Outcome::tool_calls` 按调用顺序列出
`tool.fs.read_to_string` 这样的全名，沙箱槽永远为空——回执上写的就是它。
证据在 `tests/tool_door.rs`（17 条）与 `src/host.rs` / `src/tool.rs` 的单测。

**CLI 还没接**：`script_engine.rs` 里的 qjswasm 后端仍只建沙箱引擎。

`.wasm` 侧是完整的：任何过 tinyvm 装载门的标准模块都能装载、按名调用、有预算地执行。

第一个具体锚点是 `scripts/qjs/lib/fleet.js` 的等价物，也就是本仓的
`scripts/qjs/lib/fleet.qjs`——那也是归档 `agenterm-qjs` 的门。**2026-08-25 这条到了。**

`fleet.qjs` 现在是 `fleet.js` 的**完整**移植：同样 29 个操作、同样的顺序与名字、同样的
params 形状，三份绑定（lua / js / qjs）互锁在 `tests/script_fleet_facade_parity.rs`。
它此前只港了 8 个，理由当时成立、现在不成立——引擎那时既造不出对象字面量也不会
`JSON.stringify`，19 个操作的 params 没法表达。行为也对齐了：**被拒绝的操作现在
`throw`**，跟 `fleet.js` 一样；旧版返回 `"ERR " + text`，意味着一个从 `.js` 港过来的
脚本保留着 `try`/`catch` 却什么都catch不到，错误当普通数据往下流——这正是这套栈在别处
一律拒绝的静默损坏形状。

证据是 `tests/qjs_produces_a_fleet_operation.rs`，读**真文件**（不是副本）加一段 driver，
形状照抄 `agenterm-qjs` 的 `eval_fleet_module`：字符串 payload 一条、数字 payload 一条
（`JSON.stringify` 出去的是 `{"width":320}` 而不是 `"320"`）、被拒绝时可 catch 一条。
每条都拿 `OPERATION_CATALOG` 逐字段校验 payload。同一个文件里还有一条把「目录里有多少
比例够得着」变成了等式：**现在没有一条**操作因为参数类型而写不出来（旧版量的是
「只含字符串参数的那部分」，并断言差额真实存在——数字上得去之后，它按自己文档里
预写的规则改成了全等）。清单见 [PRD 36 §归档门](../../prd/PRD_02_36_agenterm_qjswasm.md)。

## 脸：两套调用约定，一张脸

手写 `.wasm` 客人说的是 wasm 数值；`.qjs` 客人说的是编译器的 **V1 表示**——一个
JavaScript 值是一对 `(tag: i32, payload: i64)`，所以入口每个参数占两个 wasm 参数、
返回两个结果。`Value` 同时承载两者，槽在装载时记下自己是哪一套，两边的调用者都不必
学对方的 ABI。

```rust
use agenterm_qjswasm::{Engine, Guest, JsValue, Value};

let mut engine = Engine::new();

// `.qjs`：一个 JavaScript 值进，一个 JavaScript 值出。
let out = engine.run_once(
    Guest::Qjs("$0 * 2"),
    None,
    "main",
    &[Value::Js(JsValue::Number(21.0))],
)?;
assert_eq!(out.values, vec![Value::Js(JsValue::Number(42.0))]);

// 手写 `.wasm`：wasm 数值，这一路一行没变。
let out = engine.run_once(Guest::Wasm(&bytes), None, "add", &[Value::I32(40), Value::I32(2)])?;
assert_eq!(out.values, vec![Value::I32(42)]);
```

`JsValue` 是**已解析成宿主数据**的投影，不是转发的原始 pair。理由是机制性的：字符串的
payload 是指向**该槽线性内存**的指针，而 `run_once` 在返回前就把槽杀了——转发指针等于
在最常见的路径上交出一个悬垂引用。所以接缝在实例还活着的时候把它读出来。

这个形状挺得过 M4：固定下来的不是变体清单，而是**解析点**——「客人表示变成宿主数据」
只有一处。数组与对象到来时是在同一处多几个变体，不是让调用者再学一套机制。真的投影不
出来的（函数值、循环对象）走 `QjswasmError::UnsupportedValue`，那个类的含义本来就是
「客人没错，是这张脸装不下」。

约定不匹配也走同一类：把裸 wasm 数值递给 `.qjs` 槽、或把 `JsValue` 递给手写模块，
是 `UnsupportedValue` 而不是默默按位重解释。字符串**作为参数**同样被拒——那需要在客人
堆里分配，而这张脸还没有通往那个分配器的门。

**约定是在装载时记下的，从来不靠签名去猜**，所以「已经编好的 `.qjs` 产物」要有自己的
入口：`Guest::CompiledQjs(&[u8])`。一份 `.wasm` 文件不记得自己是从 `.qjs` 来的，用
`Guest::Wasm` 装回去，V1 pair 就原样过脸——字符串变成一个 tag 加一个指向马上要被丢掉的
线性内存的指针。任何「先编译到盘、以后再跑」的形状（`pack` 产物、缓存、网上取来的客人）
都需要这个变体。它不多给任何权力：同一道装载校验、同一套 `Limits`、同一扇门，只是槽记
下的约定不同。

顺带，它也是**接缝那五条防线第一次可测**的原因：`read_guest_string` 的五种拒绝（指针
不是地址、头越界、体越界、非 UTF-8、根本没有线性内存）在此之前只可能被信任的编译器产物
触发，即没有任何可达的调用者。`tests/seam_attack.rs` 的 `a_hostile_*` 五条现在真的打得到。

`spawn` / `call` 分开，因为 tinyvm 的 `Instance` 是**持久**的：装一次、调多次，
每次顶层调用拿一份新鲜的 `max_steps` 预算。一次性客人用 `run_once`。

每次调用回报成本（`steps` / `peak_call_depth` / `peak_activation_slots` / `host_ops` /
`host_bytes` / `waited_ms` / `heap_pages`；失败的调用留在 `Engine::take_failed_cost`），
所以「这个脚本贵不贵」「是算得多还是等得久」是可度量的，不是靠猜。

## 隔离与预算

一份 `.wasm`（手写的或 `.qjs` 编出来的）= 一个槽 = 一份预算。槽间互不可见，
只经宿主门看世界，**一个坏槽只能弄死自己**。

| 预算 | 归属 | 触发后 |
|------|------|--------|
| `max_steps`（每次顶层调用） | tinyvm | 该次调用 trap，槽可回收，宿主活着 |
| `max_memory_pages` / `max_table_elems` | tinyvm | 装载期拒绝；运行期 `grow` 失败 → `Budget("max_memory_pages")` |
| `max_call_depth` / `max_activation_slots` | tinyvm | trap，不吃原生栈 |
| `max_stdout_bytes` | 本 crate | 截断并置 `truncated_stdout`，不静默丢 |
| `max_bridge_result_bytes` | 本 crate | 报错，**不截断**——半个 JSON 比拒绝更糟 |
| `max_result_string_bytes` | 本 crate | 报错，**不截断**——理由同上 |

`max_result_string_bytes` 2026-08-25 补上，因为宿主侧原本只有两个盖子，而接缝把
`.qjs` 返回的字符串**拷进宿主 String** 是第三块宿主分配、由客人定大小、两个盖子都不管。
在它之前唯一的上限是偶然的：默认预算下 `max_steps` 先耗尽（拼接是 O(n) 步），一旦客人
能便宜地造出大字符串、或谁调高 `max_steps`，真实上限就变成
`max_memory_pages × 64 KiB`，每次调用一份，持久槽上反复。
顺序也是分类：**先做越界检查，再看盖子**——声明长度装不进客人自己的内存是坏客人
（`Door`），把它说成预算等于让人去调一个调了也没用的数。

`max_memory_pages` 曾有一条**运行期**缺口，2026-08-25 补上，补的方式值得记一笔：
装载期超页一直是 `Load`，但运行期 `memory.grow` 被拒之后，上游 `tinyvm-qjs` 的
`__alloc` 把它降成一条裸 `unreachable`，到宿主这里与任何别的 `unreachable` 无法区分，
所以报的是 `Trap`。宿主侧靠「内存正好顶到上限」去猜会把真坏的脚本误判成预算问题，
所以本仓不猜，去上游补：`e1122ff` 让分配器放弃之前把 `FAULT_HEAP_EXHAUSTED` 写进客人
自己线性内存的第一个字，`tinyvm_qjs::guest_fault` 读回来。`Slot::explain` 在失败路径上
**先问客人再问核**，且只问 `JsV1` 槽——那个字是编译器运行时的约定，拿手写客人的第 0
字节去读预算就是同一种猜。

一条随之而来的事实，写在这里免得被当成 bug：**`.qjs` 槽的堆一旦撑爆就不会自愈。**
bump 指针在尝试 `grow` 之前就已前移，越过尽头之后任何分配都失败，四个字节也不行，
宿主没法把客人的 global 拨回去。保证只有一条：每次都报同一句
`Budget("max_memory_pages")`，不含糊。槽不自动回收——不分配的活儿照跑，回收是调用者的
决定，与 trap 同规矩。另外，门往客人堆里写的**累计**字节数没有任何盖子：十六个都在
`max_bridge_result_bytes` 之内的 1 MiB 答案就能用光 16 MiB 的默认槽。
测试见 `tests/seam_attack.rs::finding_4_running_out_of_pages_is_now_reported_as_a_budget`
与 `tests/door_attack.rs` 的三条。

**桥 panic 被门接住。** embedder 的 bridge 是宿主代码，而 `op` 由客人挑，所以脚本能把桥
导向它会 panic 的那条路；`panic = "abort"` 下那就是客人拉着进程一起死。门 catch 住，
报 `QjswasmError::Door`（带 panic 原话和当时的 op），不外泄、也不伪装成 status 1——
「能力坏了」与「能力说不」不是同一个答案。槽本身完好，下一次调用照常应答。

**`check` 与装载闸门。** `check_qjs` / `check_qjs_with` = 编译 + 用那次运行要花的预算过
`validate_wasm_with`。只 `compile_qjs` 是不够的：字面量池超过 `max_memory_pages` 的脚本
编得干干净净，跑起来才 `Load("memory page limit")`。`compile_qjs` 本身保持只编译。

## 宿主门 ABI

模块名 `"agenterm"`。这是客人能看见的**全部**世界。**两种客人共用这一张表**：手写
`.wasm` 直接导入它，`.qjs` 通过三个自由函数名落到它上面（见上面「`.qjs` 已经够得着门」，
以及 `plan/design-agenterm-qjswasm.md` §6.5）。

```text
print(ptr: i32, len: i32)                                              -> ()
fleet_call(op_ptr: i32, op_len: i32, params_ptr: i32, params_len: i32) -> i32   // status
fleet_result_len()                                                     -> i32
fleet_result(dst_ptr: i32, dst_len: i32)                               -> i32   // 写入字节数，负=目标太小
```

`status`：`0` = Ok · `1` = Err（应用级错误，是正常结果不是崩溃）· `2` = NoBridge。
状态码语义与 `agenterm-wasmcore` 一致，guest 作者只学一套。

**`agenterm.*` 之外的 import 装载期即拒，并把名字说出来。** 四件门函数客人可以只导入
一部分、或一个都不导入；但导入**别的模块名**（最典型的是
`wasi_snapshot_preview1.fd_write`）是另一回事——没人能绑它。2026-08-25 之前这种模块
`validate_wasm` 返回 `Ok(())`、`spawn` 成功、第一次调用才死在
`Trap("call to unbound imported function")`：check 放行了 execute 跑不了的东西，而且
那条 trap 一个 import 名都不报（tinyvm 是 `no_std`，文案是静态前缀）。现在两条路
给同一个答案，都在 `Door` 类里，都带名字。锁在
`tests/host_door.rs::check_and_execute_agree_that_an_unbindable_import_is_refused_at_load`。

背后就是全仓共用的 `ScriptFleetBridgeFn`。本 crate 把**这一条既有能力**暴露给 wasm
客人，不发明第二条。

### 调用不合导出的签名，是调用者的错，不是客人的错

`call` 在进客人**之前**先问一次导出的声明类型
（`WasmInstance::exported_function_handle`），三种误报因此各归各位：

| 情形 | 2026-08-25 之前 | 现在 |
|------|----------------|------|
| 导出名不存在 | `Trap("no exported function named")`，不带名字 | `NoSuchExport`，带名字 |
| 参数**个数**不对 | `Trap("function")` | `Signature`，两个数都报；`.qjs` 槽按 JavaScript 参数个数报，不按 wasm 字数 |
| 参数**类型**不对 | **不报**——`(param i32)` 收 `I64` 返回 `Ok([I64(..)])`，与导出自己的 `(result i32)` 矛盾；只有客人真去用那个值才变成 trap | `Signature`，报参数序号和两个类型 |
| 结果类型这张脸装不下 | 客人跑完之后才 `UnsupportedValue`，它一路上打印的输出被一起丢掉 | 进客人之前就拒，什么都没产生，也就没得丢 |

最后一行顺带修掉了一个丢输出的洞。**残留的代价现在是明说的**：客人已经跑起来之后才失败
的调用（trap、预算、V1 pair 畸形），它打印的东西仍然会丢——这条写在 `Slot::call` 上，
是有意的，因为把它留到**下一次**调用的 `Outcome` 里比丢掉更糟（张冠李戴）。

### 为什么是两趟拷贝

`agenterm-wasmcore` 的六参数单次调用里，宿主拿到结果后回调 guest 导出的
`wasmcore_alloc` 要一块 buffer。**那条路在 tinyvm 上走不通**，理由是机制性的：

tinyvm 的宿主回调签名是 `Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError>`——
回调**持着线性内存的 `&mut`**，而回调进 guest 需要 `Instance::invoke_by_name(&mut self)`。
安全 Rust 里这两者不能同时成立，即**宿主回调内部无法重入 guest**。这不是 tinyvm 的
缺陷，是它「无 JIT + 显式调用栈 + 上限在核」的必然结果。

所以 `fleet_call` 只回 status，字节暂存在该槽宿主侧的 pending buffer；guest 自己问
长度、自己分配、再让宿主拷进来。多两次跨界，换来零重入、宿主不要求 guest 导出分配器，
且与 tinyvm iOS 桥既有的 two-pass 手法同源。

## 与相邻 crate 的关系

| 面 | crate | 引擎 | 信任模型 |
|----|-------|------|----------|
| `.qjs` / `.wasm`（本 crate） | `agenterm-qjswasm` + `tinyvm-qjs` | tinyvm，**无 JIT**，自研编译器 | 不信任字节 |
| `.js` / `.mjs` | ~~`agenterm-qjs`~~ | ~~rquickjs → QuickJS C~~ | **2026-08-28 已归档**：crate 摘除，`rquickjs` 出依赖树。这两个扩展名现在不选任何引擎 |
| ~~`.wasm`~~ | ~~`agenterm-wasmcore`~~ | ~~wasmtime + WASI p1，**JIT**~~ | **已于 2026-08-28 归档**；`.wasm` 现在不路由到任何引擎 |

本 crate **不改** `.js` / `.mjs` / `.wasm` 的默认路由。要让它接管 `.wasm`，显式设
`AGENTERM_SCRIPT_BACKEND=qjswasm`。

## 开发

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo test  -p agenterm-qjswasm
cargo clippy -p agenterm-qjswasm --all-targets -- -D warnings
```

依赖 `tinyvm` 与 `tinyvm-qjs` 都是**同一个私有仓的 git 依赖**，钉同一个 rev。仓根 `.cargo/config.toml` 里的
`[net] git-fetch-with-cli = true` 是**必需**的：cargo 内置的 libgit2 客户端拿不到
GitHub 私有仓凭据，实测报 `failed to receive HTTP 200 response: got 401`。

`wat` 只是 **dev-dependency**，用来把对抗性客人写成可读的 `.wat` 文本。产品自己的
wasm 编码器在上游 `tinyvm-qjs/src/encode.rs`——刻意不引 `wasm-encoder`，因为产物必须过
tinyvm 的严格装载门（canonical function expression、strict memarg alignment、strict
i64 signed-LEB range…），那份正确性要自己负责。

语言子集的验收测（能编什么、拒什么、诊断怎么说）跟编译器一起在上游
`crates/tinyvm-qjs/tests/`。本 crate 的 `tests/qjs_guest.rs` 测接缝：`.qjs` 端到端
过槽、两套调用约定、JS 值投影与字符串解析、编译失败自成一类、产物过本 crate 的装载门、
扩展名路由；`tests/qjs_door.rs` 测 `.qjs` 那一侧的门。

其中 `the_capability_claims_in_this_crates_own_copy` 是**上面那张能力表的锁**：本
README 与 PRD 36 用 agenterm 自己的口径做能力声明，所以那些声明必须由一条会跑的测试
兜住，而不是靠读上游源码。"M0，只有整数表达式"就是这样漂成假话的。

**这把锁今天还没盖满整张表**，说清楚免得读的人高估它。有锁的：「能跑」那一栏的绝大部分，
以及门那张表里的骨干（四个 import 的字节、三条状态路、`print` 求值为 `undefined`、门外
名字被拒）——都在 `qjs_guest.rs` / `qjs_door.rs` 里。**没锁的**：「明确拒绝」那一栏只有
六条源码进了 `a_source_outside_the_subset_is_a_compile_error_not_a_load_error`
（带标签的模板、默认参数、数组 elision、数字分隔符、`class`、`new`），其余二十来条、
那条诊断缺口、以及门那张表里的编译期参数检查 / 遮蔽 / 死分支仍发射 import 这几行，都是
逐条编译执行记录下来的：跑过，但上游一旦放宽，只有那六条会自己喊出来。

**这条警告已经兑现两次。** `e1122ff → 14a641a` 时，那张列表里的 `?:` 与对象字面量都被
上游实现了，测试立刻转红——把这次抬 rev 从「改两行 `Cargo.toml`」变成了「整表复测」，
正是它存在的理由。要么把剩下的补进那条测试，要么下次抬 rev 时把这张表整个复测一遍。
