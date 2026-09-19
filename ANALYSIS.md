# combine choice、checkpoint 与错误合并分析

本文只描述当前源码与新增测试能核对的行为，不改变公开语义。

- 准备命令：`cargo build --all-targets`（准备阶段，不计入验收）。
- 验收命令：在仓库根目录运行 `cargo test --quiet understanding`，退出码应为 0；新增回归测试名为 `understanding_choice_checkpoint_keeps_farthest_error_position`。

## 1. 最小模型

两个分支共享前缀 `a`：

- 分支 1：`char('a').with(char('x'))`，可缩写为 `AX`。
- 分支 2：`char('a').with(char('y'))`，可缩写为 `AY`。
- 输入：`ac`。

`choice` 在开始前保存 `before_position = input.position()` 与 `before = input.checkpoint()`；子解析器返回 `PeekErr` 时先 `input.reset(before.clone())`，再尝试下一支，元组入口见 `src/parser/choice.rs:259`、`src/parser/choice.rs:260`、`src/parser/choice.rs:261`，重置并递归下一支见 `src/parser/choice.rs:189`、`src/parser/choice.rs:190`、`src/parser/choice.rs:191`。slice/数组路径在每轮循环开头重置，见 `src/parser/choice.rs:410`、`src/parser/choice.rs:411`。

`ParseResult` 的四态是 `CommitOk`、`PeekOk`、`CommitErr`、`PeekErr`，含义见 `src/error.rs:945`–`src/error.rs:955`；`Commit` 也明确说明 `CommitErr` 阻止尝试其他分支、`PeekErr` 允许其他分支，见 `src/error.rs:207`–`src/error.rs:220`。

### 1.1 不加 `attempt`

`AX` 消费前缀 `a` 后，在第 2 列遇到 `c` 而不是 `x`。sequence 中前一个 parser 的成功会把后续失败提升为 committed error：`Then` 在前置 parser committed 时把后续 `PeekErr` 转为 `CommitErr`，见 `src/parser/sequence.rs:685`–`src/parser/sequence.rs:691`；tuple 路径同样以 `first_empty_parser` 记录已经有 committed 成功，并在后续 `PeekErr` 时回到 `add_errors`，见 `src/parser/sequence.rs:180`–`src/parser/sequence.rs:190`、`src/parser/sequence.rs:210`–`src/parser/sequence.rs:219`、`src/parser/sequence.rs:135`–`src/parser/sequence.rs:137`。

因此 `choice` 收到 `CommitErr` 后立即返回，不进入 `reset + next branch` 路径；元组逻辑见 `src/parser/choice.rs:180`–`src/parser/choice.rs:188`，slice 逻辑见 `src/parser/choice.rs:413`–`src/parser/choice.rs:417`。

输入 `ac` 的结果是失败，最终错误位置为第 2 列，错误包含：

- `Unexpected('c')`
- `Expected('x')`

这由新增测试 `understanding_choice_checkpoint_keeps_farthest_error_position` 锁定，见 `tests/understanding.rs:34`。

### 1.2 加 `attempt`

将分支写成 `choice((attempt(AX), attempt(AY)))` 时，`attempt` 的 `parse_mode_impl` 会把子解析器的普通 `CommitErr` 改成 `PeekErr`，见 `src/parser/combinator.rs:128`–`src/parser/combinator.rs:135`。唯一例外是 partial input 中的 `unexpected end of input`：它保持 `CommitErr`，见 `src/parser/combinator.rs:131`–`src/parser/combinator.rs:133`。

所以在完整输入 `ac` 上，`attempt(AX)` 的失败会变成 `PeekErr`；`choice` 把 stream reset 回分支开始位置后再运行 `AY`。若第二支也失败，两个错误通过 `ParseError::merge` 合并，入口见 `src/parser/choice.rs:130`–`src/parser/choice.rs:136` 与 `src/parser/choice.rs:436`–`src/parser/choice.rs:439`。

### 1.3 在中间 commit

“中间 commit”指分支尚未结束，但已经有一个 token/range 成功提交；本模型就是读完 `a` 后在读 `x` 处失败。它与零宽 commit 不同：此时位置确实从第 1 列推进到第 2 列，错误位置代表已经消费过的最远点，而不是分支起点。

`choice` 用位置判断 partial 恢复状态：如果 `CommitErr` 后 `input.position() != before_position`，就保存当前子 parser 的 partial state，见 `src/parser/choice.rs:180`–`src/parser/choice.rs:186`；slice 路径用 `index_state = i + 1` 记录已提交分支，见 `src/parser/choice.rs:413`–`src/parser/choice.rs:416`。这决定后续 partial decode 是重新跑所有分支，还是只恢复已提交的那一支。


## 2. Checkpoint 如何穿过各类 stream

`Stream` 只是 `StreamOnce + ResetStream + Positioned` 的组合，见 `src/stream/mod.rs:115`–`src/stream/mod.rs:169`。`ResetStream` 定义可 clone 的 checkpoint 与 `reset`，见 `src/stream/mod.rs:142`–`src/stream/mod.rs:151`。

- slice：`&str`、`&[T]` 使用 `clone_resetable!`，checkpoint 就是当前剩余 slice 的克隆，reset 通过覆盖引用实现，见 `src/stream/mod.rs:31`–`src/stream/mod.rs:47` 与 `src/stream/mod.rs:153`–`src/stream/mod.rs:156`；`&str` 的位置来自剩余指针/字节偏移，见 `src/stream/mod.rs:481`–`src/stream/mod.rs:505`。
- easy stream：`easy::Stream<S>` 直接委托底层 `S::Checkpoint`，reset 时转换底层错误类型，见 `src/stream/easy.rs:812`–`src/stream/easy.rs:827`；它的 `Error` 是 `easy::ParseError<S>`，见 `src/stream/easy.rs:830`–`src/stream/easy.rs:844`。
- position stream：checkpoint 同时保存底层 input checkpoint 与 positioner checkpoint，reset 先恢复 input，再恢复 positioner，见 `src/stream/position.rs:403`–`src/stream/position.rs:422`。
- buffered stream：checkpoint 只保存逻辑 offset，见 `src/stream/buffered.rs:38`–`src/stream/buffered.rs:46`；如果回退距离超过环形缓冲保留范围，reset/后续 uncons 会产生 `Backtracked to far`，见 `src/stream/buffered.rs:48`–`src/stream/buffered.rs:59` 与 `src/stream/buffered.rs:126`–`src/stream/buffered.rs:129`。
- decoder/BufReader：decode 层每次从 buffer 构造 `MaybePartialStream`，见 `src/stream/mod.rs:1423`–`src/stream/mod.rs:1432`；同步 `Decoder` 读到 0 字节才设置 EOF，见 `src/stream/decoder.rs:137`–`src/stream/decoder.rs:150`；`Bufferless` 直接查看或推进 `BufReader` 内部 `BytesMut`，见 `src/stream/buf_reader.rs:332`–`src/stream/buf_reader.rs:353`。

这些实现共享“保存可恢复位置、失败后 reset、错误仍保留原位置”的概念，但保存的内容不同：slice 是剩余引用，position stream 是 input+positioner，buffered stream 是 offset，decoder 还要额外保存 parser partial state。

## 3. 错误合并：position、expected 与 EOF

`easy::Errors::merge` 只比较位置：位置较前者被丢弃；位置相同时才把 `other.errors` 去重后追加到 `self.errors`，见 `src/stream/easy.rs:696`–`src/stream/easy.rs:718`。去重由 `add_error` 的 `PartialEq` 判断完成，见 `src/stream/easy.rs:673`–`src/stream/easy.rs:684`。

因此：

1. **最远错误优先**：分支 A 在位置 2 失败、分支 B 在位置 0 失败时，最终只保留位置 2 的错误；不是“最后尝试的分支覆盖前面的分支”。
2. **expected 集合**：只有错误位置相同，两个分支的 `Expected` 才会合并；不同位置时较近分支的 expected 连同错误一起被丢弃。
3. **position 来源**：错误创建时记录 `input.position()`；例如 range parser 在解析前保存 position，见 `src/parser/range.rs:50`–`src/parser/range.rs:59`。checkpoint reset 只改变后续读取用的 stream，不回写已经构造出的 `ParseError.position`。
4. **EOF**：完整 stream 的 uncons 错误由 `wrap_stream_error` 包成 `PeekErr`，见 `src/stream/mod.rs:246`–`src/stream/mod.rs:258`；顶层 `parse_stream` 在最终 `PeekErr` 时 reset 到起点，若不能再 `uncons` 就追加 `end_of_input`，见 `src/parser/mod.rs:145`–`src/parser/mod.rs:157`；partial 版本同理，见 `src/parser/mod.rs:215`–`src/parser/mod.rs:226`。
5. **partial input**：`PartialStream::is_partial()` 恒为 true，见 `src/stream/mod.rs:791`–`src/stream/mod.rs:807`；同一底层错误在 partial 模式下由 `wrap_stream_error` 变为 `CommitErr`，见 `src/stream/mod.rs:253`–`src/stream/mod.rs:257`（同函数 `src/stream/mod.rs:246`–`src/stream/mod.rs:258`）。range helper 还会在 partial EOF 时强制返回 `CommitErr`，让 decoder 请求更多数据，见 `src/stream/mod.rs:308`–`src/stream/mod.rs:319` 与 `src/stream/mod.rs:339`–`src/stream/mod.rs:374`。

## 4. partial decode 与异步 Pending

`decode` 先保存 start checkpoint，再调用 `parse_with_state`，见 `src/stream/mod.rs:1306`–`src/stream/mod.rs:1317`。当错误是 EOI 且 input 仍标记为 partial 时，它返回 `Ok((None, consumed_distance))`，表示需要更多输入，而不是解析失败，见 `src/stream/mod.rs:1318`–`src/stream/mod.rs:1324`。Tokio 版本在最终 EOF 且未消费数据时还会返回 `Ok((None, 0))` 表示流结束，见 `src/stream/mod.rs:1354`–`src/stream/mod.rs:1368`。

同步 decode 宏的循环为：

1. 用当前 buffer 构造 `MaybePartialStream(buffer, !end_of_input)`，见 `src/stream/mod.rs:1423`–`src/stream/mod.rs:1432`。
2. 如果 `decode` 返回 `None`，推进已 committed 的字节，然后调用 `__before_parse` 读取下一块，见 `src/stream/mod.rs:1443`–`src/stream/mod.rs:1457`。
3. futures 0.3 版本在相同位置 `await __before_parse_async`，见 `src/stream/mod.rs:1539`–`src/stream/mod.rs:1554`。

异步 `Poll::Pending` 不是 `ParseResult`，也不会进入 choice 的分支决策。futures reader 的扩展逻辑在 `poll_extend_buf` 返回 `Ready` 前只被 future 等待，见 `src/stream/buf_reader.rs:493`–`src/stream/buf_reader.rs:524`；decode future 本身只是轮询这个闭包，见 `src/future_ext.rs:20`–`src/future_ext.rs:28`。也就是说：

- Pending 发生在 parser 之前：没有分支被判定成功或失败。
- Pending 后再次 poll：parser 是否恢复、恢复哪一支，取决于上一轮 parser 存下的 `PartialState`。
- 若上一轮已经有分支 committed 并以 partial EOI 失败，choice 会保存该分支状态；元组逻辑见 `src/parser/choice.rs:180`–`src/parser/choice.rs:186`，slice 恢复逻辑见 `src/parser/choice.rs:400`–`src/parser/choice.rs:407`。

## 5. 状态转换表

| 当前情形 | 子结果 | 输入是否被 reset | 是否尝试下一支 | 后续状态/最终结果 |
|---|---|---:|---:|---|
| 完整输入，子 parser 未消费即失败 | `PeekErr` | 是 | 是 | 合并候选错误；元组见 `src/parser/choice.rs:189`–`src/parser/choice.rs:201` |
| 完整输入，子 parser 已消费后失败 | `CommitErr` | 否 | 否 | 直接返回 committed error；见 `src/parser/choice.rs:180`–`src/parser/choice.rs:188` |
| 完整输入，`attempt` 包住已消费失败 | 子结果先为 `CommitErr`，再变 `PeekErr` | 是 | 是 | `attempt` 转换见 `src/parser/combinator.rs:128`–`src/parser/combinator.rs:135` |
| partial 输入，`attempt` 包住的错误是 EOI | 保持 `CommitErr` | 否 | 否 | 请求更多输入；见 `src/parser/combinator.rs:131`–`src/parser/combinator.rs:133` |
| partial 输入，首个 `CommitErr` 且 position 推进 | `CommitErr` | parser 内部恢复 state 时另行处理 | 否 | 保存子 parser state；见 `src/parser/choice.rs:180`–`src/parser/choice.rs:186` |
| partial 输入，slice choice 已有 `index_state != 0` | 恢复指定子 parser | 不重新扫描分支 | 否 | 直接恢复该 index；见 `src/parser/choice.rs:400`–`src/parser/choice.rs:407` |
| 所有分支均 `PeekErr` | 合并错误 | 每轮尝试前 reset | 已全部尝试 | 同位置错误去重合并；见 `src/parser/choice.rs:149`–`src/parser/choice.rs:164`、`src/stream/easy.rs:696`–`src/stream/easy.rs:718` |
| 任一分支成功 | `CommitOk`/`PeekOk` | 否 | 否 | 返回成功；元组见 `src/parser/choice.rs:177`–`src/parser/choice.rs:179`，slice 见 `src/parser/choice.rs:443`–`src/parser/choice.rs:446` |
| choice 数组/slice 为空 | 无候选错误 | 不适用 | 否 | 构造 “parser choice is empty”；见 `src/parser/choice.rs:449`–`src/parser/choice.rs:454` |
| buffered stream 回退超出缓存 | reset 或 replay 返回错误 | 失败 | 不适用 | `Backtracked to far`；见 `src/stream/buffered.rs:48`–`src/stream/buffered.rs:59`、`src/stream/buffered.rs:126`–`src/stream/buffered.rs:129` |
| 异步 reader 返回 Pending | 不产生 parse result | 不适用 | 不适用 | future 等待；见 `src/stream/buf_reader.rs:493`–`src/stream/buf_reader.rs:524` |

## 6. 五个风险点与最小复现形态

下列“预期错误”是当前代码路径下应观察到的错误；第一项由源码核对，第二项由仓库现有测试覆盖，后三项是源码能推出的风险形态。除特别说明外，都使用公开 parser/stream API，不复制 `choice` 内部实现。

### 风险 1：零宽 `Commit` 也会锁死分支

- 最小 parser：
  - `P0`：用公开的 `parser(|input| Ok(((), Commit::Commit(()))))` 返回零宽 `CommitOk`。
  - 完整选择：`P0.with(char('x')).or(char('z'))`。
- 输入：`c`。
- 预期错误：位置 1；`Unexpected('c')`、`Expected('x')`；分支 `char('z')` 不应被尝试。
- 依据：`Commit::Commit` 会在 sequence 中把后续 `PeekErr` 提升为 `CommitErr`，见 `src/parser/sequence.rs:685`–`src/parser/sequence.rs:691`；tuple 的 `first_empty_parser` 也把任何 `CommitOk`（包括零宽）记录为已有 parser 成功，见 `src/parser/sequence.rs:180`–`src/parser/sequence.rs:190`。
- 风险：commit 状态按“状态”而不是“实际消费字节数”传播；零宽 parser 的作者如果把前瞻或动作标成 committed，会阻止 choice 回退。

### 风险 2：`buffered::Stream` 的 checkpoint 受 lookahead 容量限制

- 最小 parser：`choice([attempt(string("apple")), attempt(string("orange")), attempt(string("ananas"))])` 外包 `sep_by(..., char(','))` 的形态可参考现有测试。
- 输入：`apple,apple,ananas,orangeblah`。
- 构造：`buffered::Stream::new(easy::Stream(position::Stream::new(IteratorStream::new(iter))), 1)`。
- 预期错误：`easy::Error::Message("Backtracked to far")`。
- 依据：checkpoint 只是 offset，见 `src/stream/buffered.rs:42`–`src/stream/buffered.rs:46`；容量不足时 reset/ replay 报错，见 `src/stream/buffered.rs:48`–`src/stream/buffered.rs:59` 与 `src/stream/buffered.rs:126`–`src/stream/buffered.rs:129`。
- 仓库中的公开 API 回归测试：`tests/buffered_stream.rs:48`–`tests/buffered_stream.rs:73`。

### 风险 3：`attempt` 在 partial EOF 下不解锁分支

- 最小 parser：`choice((attempt(string("ab")).message("AB"), attempt(string("ac")).message("AC")))`。
- 输入：partial `"a"`（用 `PartialStream("a")` 或 decode 中 `end_of_input=false`）。
- 预期错误：partial 直接解析时为位置 1 的 `Unexpected("end of input")`，并携带分支 AB 的 message；走 `decode` 时转为 `Ok((None, consumed))`，不会尝试 AC。
- 依据：partial uncons 错误变成 `CommitErr`，见 `src/stream/mod.rs:253`–`src/stream/mod.rs:257`；`attempt` 对 partial EOI 特判并保留 `CommitErr`，见 `src/parser/combinator.rs:128`–`src/parser/combinator.rs:135`；`decode` 再把 EOI+partial 转成 `Ok((None, _))`，见 `src/stream/mod.rs:1318`–`src/stream/mod.rs:1324`。
- 风险：完整输入下 `attempt` 表示“失败可回退”；partial EOF 下同一个 parser 表达的是“此分支可能还没读完，先保留并等待数据”。这是有意的恢复机制，但使用者若按完整输入直觉理解，可能误以为下一支会立刻运行。

### 风险 4：BufReader/decoder 跨缓冲边界恢复时，状态绑定的是“原分支 + 原字节地址”

- 最小 parser：`choice((attempt(range(&b"ab"[..])), attempt(range(&b"ac"[..]))))`。
- 输入分两块：第一块 `a`，第二块 `c`；第一块返回 `Ok((None, 1))` 后，外层解码器允许移除已经 committed 的 `a`。
- 第二次 decode 的输入：新 buffer `c`，`PartialState` 沿用第一次保存的状态。
- 预期错误：错误发生在恢复出的第一分支上；`Errors.position` 是旧 `a` 的 pointer offset，错误集合包含 `Expected([97,98])` 与 `Expected([97,99])`，同时包含 EOI/`c` 的 unexpected 信息。
- 依据：第一次 committed EOF 时 choice 保存分支状态，见 `src/parser/choice.rs:180`–`src/parser/choice.rs:186`；恢复时 slice choice 直接返回指定子 parser，不重新扫描分支，见 `src/parser/choice.rs:400`–`src/parser/choice.rs:407`；`MaybePartialStream` 包装的是当前 buffer slice，见 `src/stream/mod.rs:1423`–`src/stream/mod.rs:1432`；slice checkpoint 克隆的是剩余 slice 指针，见 `src/stream/mod.rs:153`–`src/stream/mod.rs:156`；同位置 merge 不去区分新旧 pointer，只比较 `Position: Ord`，见 `src/stream/easy.rs:696`–`src/stream/easy.rs:718`。
- 已核对的安全前提：正常使用内部 `Buffer` 的 `decode!` 会把未形成消息的数据保留在 decoder buffer 中；同步宏按 `removed` 推进，见 `src/stream/mod.rs:1443`–`src/stream/mod.rs:1457`。因此风险成立的条件是外部手动管理 `Bufferless/BufReader`、自定义 codec 或跨 buffer 拷贝时错误移动了仍被 partial state 依赖的字节。
- 标注：该“错误集合与 pointer 位置混合”的具体现象由我在本次分析中的临时探针观察到，未加入永久测试；源码能核对状态绑定与 pointer checkpoint 机制，但具体错误展示仍建议用临时复现确认。

### 风险 5：异步 `Poll::Pending` 与 parser 分支错误处在不同层

- 最小 reader：第一次 `poll_read` 返回 `Poll::Pending` 并唤醒 waker，第二次返回完整 `ac`。
- 最小 parser：`choice((attempt(bytes(b"ab")), attempt(bytes(b"ac"))))`，经 `decode_futures_03!` 或 `decode_tokio!` 解码。
- 预期：Pending 时没有 parse error，也不会选择或放弃分支；future 完成后 parser 才首次/继续运行。若 Pending 之前已经有 committed partial 结果，则恢复规则与风险 4 相同。
- 依据：异步读取在 `poll_extend_buf` 中 `ready!(read.poll_read(...))`，Pending 直接向上传播，见 `src/stream/buf_reader.rs:502`–`src/stream/buf_reader.rs:523`；decode future 只轮询读取闭包，见 `src/future_ext.rs:20`–`src/future_ext.rs:28`；futures decode 宏在读取完成后才构造 stream 并调用 `decode`，见 `src/stream/mod.rs:1521`–`src/stream/mod.rs:1531`，Pending 后的下一轮 await 见 `src/stream/mod.rs:1546`–`src/stream/mod.rs:1554`。
- 风险：`Poll::Pending` 不能用于表示“parser 当前分支失败，请尝试下一支”；它只表示 I/O 尚未就绪。要让 choice 重新考虑分支，必须在 parser 层得到可回退的 `PeekErr`，或在尚未 committed prefix 时保持 `removed=0`。

## 7. 新增最小测试

新增测试：`tests/understanding.rs:34` 的 `understanding_choice_checkpoint_keeps_farthest_error_position`。

它只通过公开 API 构造：

```rust
choice((
    char('a').with(char('x')),
    char('a').with(char('y')).message("last tried branch"),
))
```

输入 `ac` 时：

1. 第一支消费 `a` 后在位置 2 失败。
2. choice reset 到起点，第二支在位置 1 因为首字符不是 `a` 而失败。
3. `easy::Errors::merge` 比较位置后保留位置 2 的第一支错误。
4. 因而最终断言是位置 2，且只有 `Unexpected('c')` 与 `Expected('x')`，证明错误位置来自“消费更远的分支”，而不是最后尝试的第二支。

这个测试不依赖 `Choice`、tuple state、slice state 或 `Tracked.offset` 等内部类型。

## 8. 可复跑命令

```sh
# 准备阶段：构建全部 target；不计入演示/验收
cargo build --all-targets

# 验收：必须从仓库根目录直接运行，退出码为 0
cargo test --quiet understanding
```

在当前 Cargo 版本中，`--quiet` 以一个字符表示每个成功用例；为避免 libtest 隐藏成功用例输出，新增用例在断言前通过本地系统 shell 打印一次名称。实际验收输出包含：

```text
running 1 test
understanding_choice_checkpoint_keeps_farthest_error_position
.
test result: ok. 1 passed; 0 failed; ...
```

该输出只运行本地 `/bin/sh`（Windows 下为 `cmd`）的 `echo`，不启动服务、不访问网络、不读取环境变量。

这些命令不需要外部服务、公网访问或手工设置环境变量；默认 feature 已包含 `std`，见 `Cargo.toml` 的 feature 默认值。

## 9. 已确认事实与单独标注的推断

**已由源码或测试确认**

- `PeekErr` reset 后继续下一支，`CommitErr` 立即停止：`src/parser/choice.rs:180`–`src/parser/choice.rs:201`。
- slice choice 每轮 reset，committed 时记录 index：`src/parser/choice.rs:396`–`src/parser/choice.rs:417`。
- `attempt` 只在非 partial-EOI 时把 `CommitErr` 转为 `PeekErr`：`src/parser/combinator.rs:128`–`src/parser/combinator.rs:135`。
- easy error 同位置合并、不同位置取最远：`src/stream/easy.rs:696`–`src/stream/easy.rs:718`。
- 最远错误位置回归由新增测试锁定：`tests/understanding.rs:34`。
- partial EOI 被 decoder 转成 “需要更多输入”：`src/stream/mod.rs:1318`–`src/stream/mod.rs:1324`。
- 异步 Pending 在 I/O future 层传播，不是 parser 的 `ParseResult`：`src/stream/buf_reader.rs:502`–`src/stream/buf_reader.rs:523`、`src/future_ext.rs:20`–`src/future_ext.rs:28`。
- buffered stream 超距回退会报 `Backtracked to far`：`src/stream/buffered.rs:48`–`src/stream/buffered.rs:59`。

**单独标注的推断/观察**

- 风险 4 的精确错误数组与 pointer offset 组合来自本次临时探针，不是永久回归测试；源码可核对其机制，但如果要把错误展示也作为稳定保证，应另加测试或公开文档承诺。
- 风险 1、3、5 是“容易误用或误读的边界行为”，本文不把它们断言为实现 bug；它们均与当前源码一致，且不建议在未进行公开语义设计前修改。
