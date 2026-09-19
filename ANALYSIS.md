# combine `choice` 的回溯决策分析:checkpoint、Consumed/Empty 与错误合并

本文分析 combine 4.6.8 中 `choice`/`or` 如何根据 stream checkpoint、`Commit`/`Peek`
(即 Parsec 文献中的 `Consumed`/`Empty`)状态与 `ParseError` 合并来决定是否尝试下一分支。
除"推测与未验证项"一节外,每条断言都可由所引源码行或 `tests/understanding.rs` 中
的新增测试逐条核对。本文不修改任何公开语义,只新增 `ANALYSIS.md` 与
`tests/understanding.rs`。

## 概念映射

- `ParseResult` 四态:`CommitOk` / `PeekOk` / `CommitErr` / `PeekErr`
  (src/error.rs:945-960)。`Commit*` 对应 `Consumed`,`Peek*` 对应 `Empty`。
- `ResetStream::checkpoint` / `reset`(src/stream/mod.rs:143-151)。`&str`、`&[T]`
  等 slice 的 checkpoint 就是自身的廉价克隆(src/stream/mod.rs:152-157);
  `easy::Stream` 与 `PartialStream` 只是透传底层 checkpoint
  (src/stream/easy.rs:812-827,src/stream/mod.rs:774-789);
  `buffered::Stream` 的 checkpoint 是 `usize` 偏移,回退超出环形缓冲会报
  "Backtracked to far"(src/stream/buffered.rs:44-58)。
- 序列内部的提交标志按 `Peek <> Commit -> Commit` 合并(src/error.rs:275-280,
  真值表注释见 src/error.rs:283-288)。

## 两个共享前缀的分支:三种结果

分支 A = `string("let")`,分支 B = `string("lex")`,共享前缀 `"le"`,输入 `"lex"`。

### 1. 不加 `attempt`:`choice((string("let"), string("lex")))` → 整体失败

- A 消费 `'l'`、`'e'` 后在 `'x'` 处失配。`Tokens::parse_lazy` 在已提交时返回
  `CommitErr`,且错误位置被设为该 token 序列的**起点** `start`
  (src/parser/token.rs:241, src/parser/token.rs:248-252)。
- `do_choice!` 拿到 `CommitErr` 直接向上传播,**不尝试 B**
  (src/parser/choice.rs:180-188)。
- 因为不是 `PeekErr`,顶层 `parse_stream` 的错误补全逻辑不会执行
  (src/parser/mod.rs:141-160),最终错误只有 `Unexpected('x')`,位置 1:1。
- 验证:`understanding_choice_without_attempt_stops_at_first_commit`
  (tests/understanding.rs:30)。

### 2. 加 `attempt`:`choice((attempt(string("let")), string("lex")))` → B 成功

- `attempt`(`Try`)把 `CommitErr` 降级为 `PeekErr`
  (src/parser/combinator.rs:128-137);唯一例外见"partial input"一节。
- `do_choice!` 拿到 `PeekErr` 后执行 `input.reset(before.clone())` 恢复到进入
  choice 前保存的 checkpoint(src/parser/choice.rs:189-190,checkpoint 在
  src/parser/choice.rs:260-261 建立),然后尝试 B,得到 `Ok(("lex", ""))`。
- 验证:`understanding_choice_with_attempt_backtracks_to_checkpoint`
  (tests/understanding.rs:47)。

### 3. 在中间 commit:`choice((char('l').with(attempt(string("et"))), string("lex")))` → 整体失败

- `char('l')` 消费 `'l'` 返回 `CommitOk`;`attempt(string("et"))` 失败只产生
  `PeekErr`,但序列按 `Commit <> Peek -> Commit` 合并(src/error.rs:275-280),
  分支整体仍是 `CommitErr`,choice 不尝试 B。
- 错误报告在 `"et"` 的起点(1:2);`parse_committed` 的 PeekErr 补全会往错误里加
  `Unexpected('e')` 与 `Expected("et")`(src/parser/mod.rs:1124-1151)。
- 验证:`understanding_commit_in_middle_still_commits`
  (tests/understanding.rs:57)。

## 状态转换表(单个分支在 `do_choice!` 中的去向)

| 分支结果 | 含义 | choice 的行为 | 错误去向 |
|---|---|---|---|
| `CommitOk(x)` | 已提交且成功 | 立即返回 `CommitOk(x)` | — |
| `PeekOk(x)` | 未提交成功(可零宽) | 立即返回 `PeekOk(x)`,后续分支不可达 | — |
| `CommitErr(e)` | 已提交且失败 | 不尝试后续分支;若 `input.position() != before_position` 则把该分支记入 partial state 供 `parse_partial` 恢复(src/parser/choice.rs:180-188, 263-281) | `e` 原样向上传播,不做合并 |
| `PeekErr(e)` | 未提交失败 | `reset(checkpoint)` 后尝试下一分支(src/parser/choice.rs:189-201) | `e` 暂存,全部失败后按 `merge!` 两两合并(src/parser/choice.rs:130-137, 149-164) |

数组/切片版本走 `slice_parse_mode`,语义相同:逐分支 `reset` 到同一 checkpoint,
`CommitErr` 记录 `index_state` 后立即返回(src/parser/choice.rs:383-460)。

## 各因素如何影响决策

### 最远错误(furthest error)

所有分支都 `PeekErr` 时,`do_choice!` 用 `ParseError::merge` 合并各分支的
`Tracked` 错误(src/parser/choice.rs:149-164)。默认实现只保留后者
(src/error.rs:479-483);`easy::Errors::merge` 按 position 比较,**只保留消费
最远的那个分支的错误**,同位置才合并错误列表(src/stream/easy.rs:699-717,
委托见 src/stream/easy.rs:443-445)。因此 checkpoint 恢复后,报告位置来自消费
最远的分支,而不是最后尝试的分支——见"关键最小测试"。

### expected 集合

- 同位置合并时 `add_error` 去重后并入(src/stream/easy.rs:699-717)。
- 位置不同时,较近分支的**错误列表**(含 expected)被整体丢弃。
- 但顶层 `parse_stream`/`parse_stream_partial` 对 `PeekErr` 会 reset、peek 一个
  token 补 `Unexpected`,再调用 `self.add_error(error)`(src/parser/mod.rs:141-160,
  210-228);`Choice::add_error` 经 `add_error_choice` 遍历**所有**分支补 expected
  (src/parser/choice.rs:284-298, 375-380)。所以最终 expected 集合仍可能包含
  输掉位置竞争的分支的 `Expected` 项(测试 4 中的 `Expected('z')`)。
- `Tracked.offset`(`ErrorOffset`)控制序列中哪个子 parser 的 expected 被加入
  (src/error.rs:925-941,src/parser/choice.rs:149-164 的注释)。

### position

- 只有 `input.position() != before_position` 时,`CommitErr` 的分支才会被记入
  partial state;否则下轮 `parse_partial` 重试所有分支
  (src/parser/choice.rs:180-188)。position 的 `Ord` 是"最远错误"比较的唯一依据
  (src/stream/mod.rs:127-131)。
- `string`/`tokens` 类 parser 把错误位置设为序列**起点**而非失配点
  (src/parser/token.rs:241, 248-252),所以两个 `string` 分支的合并位置往往相同
  (都是 choice 入口),expected 集合会合并而不是互相丢弃。

### EOF

- 流耗尽时 `uncons` 返回 `end_of_input` 错误;非 partial 流上它是普通 `PeekErr`,
  参与正常合并(src/stream/mod.rs:246-259)。
- 顶层补全在 peek 不到 token 时改加 `end_of_input`(src/parser/mod.rs:150-155)。

### partial input

- `PartialStream`/`MaybePartialStream` 使 `is_partial() == true`
  (src/stream/mod.rs:756-806, 948-996)。
- partial 流上 `uncons` 耗尽(缓冲区末尾,而非真 EOF)时 `wrap_stream_error`
  返回 **`CommitErr`**(src/stream/mod.rs:246-259):choice 把它当作"已提交",
  记录 partial state 并等待更多输入,而不是尝试下一分支。
- `attempt` 对 partial 流上的 unexpected-EOF **不会**降级,`CommitErr` 原样保留
  (src/parser/combinator.rs:130-136)。
- `stream::decode` 在"unexpected EOF 且 partial"时返回 `Ok((None, consumed))`
  表示输入不足(src/stream/mod.rs:1306-1333);`decode!` 据此循环 refill
  (src/stream/mod.rs:1409-1462)。

### 异步 Pending

- 同步侧"输入不足"的等价物是 `decode` 的 `Ok(None)`;异步侧由
  `decode_tokio!` 等宏在 `Ok(None)` 后调用 `__before_parse_tokio`,其内部
  `poll_fn` 等待 `poll_extend_buf`(src/stream/decoder.rs:190-207,
  src/stream/buf_reader.rs:136-143)。`Poll::Pending` 期间 parser 的
  partial state 已保存在 `Decoder` 里,数据到达后 `parse_partial` 只恢复
  已提交的那一支(src/parser/choice.rs:263-281)。
- 证据测试:tests/async.rs:341-358 的 `choice_test`(quickcheck 随机分片 +
  WouldBlock),需要 `--features tokio-02,futures-io-03`,不在默认验收命令内。

## 五个风险点

### R1(零宽 parser)零宽分支静默遮蔽后续分支

- 最小 parser:`choice((many(digit()).map(|v: Vec<char>| v.len()), string("abc").map(|s| s.len())))`
- 输入:`"abc"`
- 预期结果:**不报错**,`Ok((0, "abc"))`。`many(digit())` 零宽成功(`PeekOk`),
  `string("abc")` 永远不可达(状态转换表第 2 行)。
- 验证:`understanding_zero_width_branch_shadows_later_branches`
  (tests/understanding.rs:101)。

### R2(中间 commit)`attempt` 只包尾部不等于可回溯

- 最小 parser:`choice((char('l').with(attempt(string("et"))), string("lex")))`
- 输入:`"lex"`
- 预期错误:位置 1:2,含 `Unexpected('x')`、`Expected("et")`,**不含**
  `Expected("lex")`。
- 验证:`understanding_commit_in_middle_still_commits`
  (tests/understanding.rs:57)。

### R3(错误合并)报告位置属于最远分支,expected 却可能来自所有分支

- 最小 parser:`choice((attempt((char('a'), char('b'), char('d'))).map(|_| ()), attempt(char('z')).map(|_| ()))`
- 输入:`"abc"`
- 预期错误:位置 1:3(第一支,消费最远);含 `Unexpected('c')`、`Expected('d')`;
  由于顶层 `add_error` 补全,也含第二支的 `Expected('z')`。
- 验证:`understanding_checkpoint_reset_keeps_furthest_error_position`
  (tests/understanding.rs:74)。

### R4(BufReader 跨缓冲边界)缓冲边界处的"假 EOF"会让 choice 提前提交

- 最小 parser:`choice(((byte(b'h'), byte(b'e'), byte(b'l'), byte(b'l'), byte(b'o')).map(|_| ()), bytes(&b"help!"[..]).map(|_| ()))`,
  经 `decode!` + `Decoder::new()`,reader 每次只给 2 字节。
- 输入:`b"help!"`(分片 `"he" | "lp" | "!"`)
- 预期错误:第一支在边界处提交,refill 后只恢复该支,在 `'p'` 处失败:
  `Unexpected(b'p')`、`Expected(b'l')`,位置为剩余缓冲内偏移 1;
  `"help!"` 分支的 `Expected(Range(..))` 不出现。
- 验证:`understanding_buffered_reader_commit_survives_buffer_boundary`
  (tests/understanding.rs:135)。
- 注:`buf_reader::BufReader`(bufferless 变体)只接入了异步 decode 宏——同步
  `decode!` 的 `__before_parse` 要求 `C: CombineSyncRead<&mut BufReader<R>>`
  (src/stream/decoder.rs:142-153),而 `Bufferless` 只为按值的 `BufReader<R>`
  实现(src/stream/buf_reader.rs:347-354);异步宏走 `advance_pin` /
  `__before_parse_tokio`(src/stream/mod.rs:1826-1831)。用法示例见
  tests/async.rs:816-821。因此本风险点用共享同一边界语义的
  `Decoder::new()`(内部 `Buffer`)演示。

### R5(异步恢复)Pending 之后不会重新评估其他分支

- 最小 parser:`many1(digit()).or(many1(letter())).skip(range(&"\r\n"[..]))`
  (即 tests/async.rs:266-274 的 `choice_parser`)。
- 输入:任意分片,如 `"12" | "3\r\n"`(第二片到达前第一片末尾触发 partial EOF)。
- 预期行为/错误:第一片末尾的 partial EOF 使 `many1(digit())` 以 `CommitErr`
  提交(见"partial input"),`decode` 返回 `Ok(None)`;异步侧 `poll_extend_buf`
  Pending 后数据到达,`parse_partial` 只续跑 digit 分支。若后续数据使该分支
  失败(如 `"12" | "x\r\n"`),整个 parse 报错,`many1(letter())` 不会被尝试,
  即使 `attempt` 包住该分支也一样(src/parser/combinator.rs:130-136)。
- 验证:tests/async.rs:341-358 `choice_test`(需
  `cargo test --features tokio-02,futures-io-03 --test async choice_test`,
  依赖 feature 开启的 tokio-02,不在默认验收命令内)。

## 关键最小测试

`understanding_checkpoint_reset_keeps_furthest_error_position`
(tests/understanding.rs:74):第一支消费 2 个 token 后在 1:3 失败,第二支(最后
尝试)在 1:1 失败。checkpoint 恢复并尝试完所有分支后,`easy::Errors::merge`
(src/stream/easy.rs:699-717)保留最远位置,断言最终 `err.position` 为 1:3 而非
1:1。测试只使用公开 API(`choice`/`attempt`/`easy_parse`),未复制
`do_choice!`/`slice_parse_mode` 的任何内部实现。

## 可复跑命令

```sh
# 准备(不计入演示)
cargo build --all-targets
# 验收:从仓库根目录运行,退出码为 0,各用例名称会打印到 stderr
cargo test --quiet understanding
# 可选:异步恢复证据(需要额外 feature,依赖未缓存时需访问 crates.io)
cargo test --features tokio-02,futures-io-03 --test async choice_test
```

## 推测与未验证项

- `Poll::Pending` 的挂起/唤醒语义由 tokio/futures 运行时决定,本仓库源码只到
  `poll_extend_buf` 的签名与调用点(src/stream/buf_reader.rs:136-143,
  src/stream/decoder.rs:190-207);"Pending 期间 partial state 不变"是从
  `Decoder` 持有 `state` 字段(src/stream/decoder.rs:51-58)推出,未用异步
  测试单独验证唤醒路径。
- R5 中 `"12" | "x\r\n"` 的具体错误文本未逐字断言;`choice_test` 只断言合法
  输入在随机分片下都能成功(tests/async.rs:341-358)。
- `slice_parse_mode` 与 tuple 版 `do_choice!` 的错误合并顺序不同
  (src/parser/choice.rs:383-460 vs 139-201),本文只验证了 tuple 版;两者
  在"最远位置优先"上是否所有边角都一致,未逐一核对。
