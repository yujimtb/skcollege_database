この文章は今後の開発方針のアドバイスを提案してもらったものです。




---
User:

plan.md
domain_algebra.md
adr_backlog.md
governance_capability_model.md
open_issues.md
README.md
runtime_reference_architecture.md

データベースの設計を一から構築し、おおまかな仕様設計までコーディングエージェントといっしょにまとめました。頭が爆発しそうなのですが、AI駆動開発においてどの程度タスクをオフロードしたり人間とAIの役割分担を行ったら良いでしょうか?
全体的な開発方針とロードマップを提案してください




---
GPT-5.4 Pro: 

まず結論だけ言うと、

> **LETHE のような「意味論・ガバナンスが重い基盤」は、**
> **設計原則と最終承認は人間が握り、**
> **実装・テスト・反復はかなりAIに寄せてよい**  
> です。

体感としては、今の段階では

- **人間主導 70 / AI 30** … コア意味論・MVP境界・倫理
- **人間主導 40 / AI 60** … adapter / projector / API / テスト / ドキュメント
- **人間主導 20 / AI 80** … ボイラープレート・CI・lint・コード整形・テスト雛形

くらいが一番安定します。

あなたの頭が爆発しそうなのは正常です。  
この仕様はすでに「考えるべきこと」をかなり考え切っていて、次の問題は**全部を一気に作ろうとすると死ぬ**ことです。  
なので、ここからは **「AIに何を渡すか」より「何を今は作らないか」** を先に決めるのが大事です。

---

# 1. まず持つべき開発方針

## 方針A: AIは「実装者」、人間は「意味論の所有者」

LETHEでは特にこれが重要です。

人間が持つべきもの:

- append-only の意味
- academic-pinned / operational-latest の境界
- authority model
- consent / filtering / review の原則
- MVPの範囲
- 公開APIの最終契約
- 本番投入可否の判断

AIに渡してよいもの:

- schema / model / table / migration の草案
- adapter 実装
- projector 実装
- API 雛形
- テストケース生成
- ドキュメント整形
- リファクタリング
- 監視・CI・lint まわり

一言でいうと、

> **“何が真か”は人間、  
> “どう作るか”はAI**

です。

---

## 方針B: 「横に広げる」のではなく「縦に切る」

今の仕様は広くて正しいです。  
でも開発は広く始めると破綻します。

なので最初は **1本の垂直スライス** に絞るべきです。  
あなたの文書にある MVP はかなり良いです。

### 最初の1本
**Google Slides + Slack → 名寄せ → 個人ページ**

これだけで以下が全部試せます。

- mutable source
- append-only capture
- snapshot/event 両方
- supplemental
- projection
- DB on DBs
- lineage
- incremental propagation
- 人に見せるAPI

つまり、**プラットフォーム全体の縮図** です。

---

## 方針C: 「仕様を書くAI」と「コードを書くAI」を分ける

おすすめの運用はこれです。

1. **人間**がタスクを小さく切る
2. **AI(設計役)** に interface / invariant / test plan を書かせる
3. **AI(実装役)** にコードを書かせる
4. **AI(レビュー役)** に law違反を探させる
5. **人間**が merge / reject を決める

同じAIでも役割を分けるだけで精度が上がります。  
特に LETHE では「仕様違反」を防ぐのが重要なので、**実装AIとレビューAIを分離**した方がよいです。

---

# 2. 人間とAIの役割分担マトリクス

以下の配分をおすすめします。

| 領域 | 主担当 | AIオフロード目安 | コメント |
|---|---:|---:|---|
| コア意味論（Observation, Projection law） | 人間 | 20% | AIは整理・比較・ADR草案まで |
| Governance / consent / review | 人間 | 20% | AIに最終判断はさせない |
| Schema設計 | 共同 | 50% | 人間が境界、AIがJSON Schema化 |
| Source contract | 共同 | 50% | authority/capture判断は人間 |
| Adapter実装 | AI寄り | 70% | API wrapper・retry・parsingはAI向き |
| Projector / SQL | AI寄り | 70% | ただし acceptance test は人間主導 |
| API / FastAPI | AI寄り | 80% | CRUD雛形、OpenAPI化はAI得意 |
| テスト生成 | AI寄り | 80% | golden test, fixture 生成を任せる |
| CI/CD / lint / typing | AI寄り | 85% | かなり任せてよい |
| 監視 / metrics / alert 雛形 | AI寄り | 70% | SLO閾値は人間が決める |
| 仕様レビュー / merge判定 | 人間 | 10% | 最終責任は必ず人間 |
| 本番運用 /インシデント対応 | 人間 | 20% | AIは補助 |

---

# 3. AIに任せてよいタスク / 任せない方がよいタスク

## AIに強く任せてよい

### 1. ボイラープレート
- Pydantic モデル
- SQLAlchemy / Prisma / migration
- FastAPI endpoint 雛形
- CLI
- config loader
- logging / metrics 雛形

### 2. Adapter系
- Google API / Slack API wrapper
- pagination / retry / backoff
- JSON → canonical payload 変換
- mock / fake source 実装

### 3. テスト系
- fixture 作成
- schema validation test
- projector replay test
- API contract test
- edge case table

### 4. ドキュメント系
- README
- architecture notes
- runbook 下書き
- OpenAPI 生成
- ADR の比較表

---

## AIに“草案まで”で止めた方がよい

- authority model の判定
- source-authoritative / dual-reference の線引き
- consent 撤回時の意味
- filtering-before-exposure の実運用
- public API の長期互換性
- deletions / takedown policy
- identity resolution の最終ルール
- write-back の許可条件

---

## AIに丸投げしない方がよい

- 倫理判断
- 個人情報 exposure の可否
- destructive operation
- production credential / secret 運用
- source-native write-back の本番許可
- “この仕様変更で law が壊れないか” の最終判断

---

# 4. LETHE向けのおすすめ開発スタイル

## 4.1 「Spec-first + Contract-test-first」

LETHEは普通のWebアプリではなく、**意味論駆動**です。  
なのでコードより先に、毎タスクで最低これを固定します。

### タスクごとに最初に書くもの
- Goal
- Invariants
- Inputs / outputs
- Non-goals
- Failure modes
- Acceptance tests

たとえば Slack adapter なら:

- Goal: Slack message を canonical Observation に変換
- Invariants:
  - append-only
  - idempotencyKey 安定
  - edited/delete は correction/retraction に正規化
- Acceptance:
  - 同一 ts の再取得で重複しない
  - thread_ts が保存される
  - user profile と join 可能

この粒度まで切ってからAIに渡すとかなり安定します。

---

## 4.2 「1タスク = 半日〜2日」で切る

AIと相性が悪いのはこういうタスクです。

- “Slack連携を全部作って”
- “Projection基盤を実装して”
- “LETHEのMVPを作って”

相性がいいのはこうです。

- `schema:slack-message` の JSON Schema と Pydantic モデルを実装
- Slack conversations.history 取得クライアントを実装
- Slack message → Observation 変換関数を実装
- idempotencyKey 生成テストを作成
- DuckDB で person-page API 用 query を作成

**巨大タスクをAIに投げると設計逸脱が増えます。**

---

## 4.3 「AI生成コードは、必ず test 付きで受け取る」

ルールとして、

> **コード単体ではなく、必ず tests / docs / migration をセットで出させる**

のがいいです。

おすすめテンプレ:

- 実装コード
- 単体テスト
- integration test
- 変更されたAPI仕様
- リスク / 未確定点メモ

---

## 4.4 「人間はレビューで“コード品質”ではなく“意味論違反”を見る」

人間レビュー時の観点は普通のコードレビューと少し違います。

### LETHEレビュー・チェックリスト
- append-only を破っていないか
- authority model を暗黙にしていないか
- read mode を混同していないか
- restricted data が filtering 前に露出していないか
- lineage が失われていないか
- idempotency が壊れていないか
- replay 不能な hidden mutable state が混入していないか

つまり、**レビューで見るのは「キレイか」より「law違反してないか」**です。

---

# 5. 現実的なロードマップ

あなたの仕様を前提に、私は以下の順を強くおすすめします。

---

## Phase 0: スコープ圧縮と凍結（3〜5日）

### 目的
「今は何を作らないか」を決める。

### 人間が決めること
- MVPは **Slides + Slack + 名寄せ + 個人ページ**
- MVPでは **write-back をやらない**
- MVPでは **public DOI / 外部公開は後回し**
- MVPでは **DuckDB + FastAPI + local/MinIO級**
- MVPでは **projection-level lineage を優先**
- row-level lineage は person-page のみ最小実装
- governance は **internal-only + restricted + audit最小** から始める

### AIに任せること
- repo 構成草案
- MVP の非目標一覧
- Phaseごとの backlog 整理
- issue / task 分解

### 完了条件
- `MVP_SCOPE.md`
- `NON_GOALS.md`
- 優先タスク一覧
- 実装順序の確定

---

## Phase 1: 最小基盤（1〜2週間）

### 作るもの
- Registry 最小実装
- Observation append API
- blob CAS
- schema validation
- Catalog 最小
- local replay 可能な Lake 保存
- FastAPI skeleton
- DuckDB projector runner

### AI主導
- DB schema
- migration
- API 雛形
- Pydantic model
- storage adapter
- pytest 雛形

### 人間主導
- Observation envelope の確定
- ordering / idempotency の確認
- schema registry の最小境界
- どこまで Lake に入れるかの最終確認

### 完了条件
- 手動 Observation を append できる
- schema validation が通る
- 再読み出しできる
- blob ref が動く

---

## Phase 2: Slack ingestion（1週間）

### 作るもの
- `schema:slack-message`
- `schema:slack-user-profile`
- Slack crawler
- initial backfill + incremental crawl
- idempotency / correction / deletion handling

### AI主導
- Slack API client
- pagination
- JSON mapping
- tests
- fixtures

### 人間主導
- Slack を `lake-authoritative` とみなす仕様確認
- edited/delete の意味論
- channel / thread / profile の capture 境界

### 完了条件
- 1 channel を backfill できる
- 以後の差分取得が動く
- user profile が取れる

---

## Phase 3: Google Slides ingestion（1〜2週間）

### 作るもの
- `schema:workspace-object-snapshot` or `schema:document-snapshot` concretization
- presentations.get / revisions / export
- native + render snapshot capture
- deck revision watermark

### AI主導
- Google API wrapper
- retry / backoff
- export pipeline
- attachment role mapping
- tests with mocks

### 人間主導
- capture 粒度
- revision identity の扱い
- native vs render の canonical boundary
- deleted / inaccessible deck の扱い

### 完了条件
- 1 deck を revision 単位で取り込める
- native + PDF/PNG 保存が動く
- revision 変化検知が動く

---

## Phase 4: 名寄せ Projection（1〜2週間）

### 作るもの
- `proj:person-resolution`
- candidate store（Supplemental）
- human confirm / reject の最小経路
- email match / display name match / mention candidate の3段階

### AI主導
- projector SQL/Python
- fuzzy match 実装
- candidate export
- review queue 雛形
- test fixture

### 人間主導
- confidenceの意味
- confirmed / candidate / rejected の定義
- human review ルール
- canonical person id の付与戦略

### 完了条件
- Slack user と Google identity を少数サンプルで紐づけられる
- candidate と confirmed を区別できる
- build 再実行で安定する

---

## Phase 5: 個人ページ Projection / API（1週間）

### 作るもの
- `proj:person-page`
- `/api/persons`
- `/api/persons/{id}`
- `/slides`, `/messages`, `/timeline`
- 最小 lineage 表示

### AI主導
- query
- API implementation
- OpenAPI
- integration tests
- sample frontend JSON

### 人間主導
- 画面要件
- どの relation を “related” と呼ぶか
- exposure policy
- 表示項目の優先順位

### 完了条件
- 人物ごとに slides / messages / timeline が見える
- lineage が少なくとも observation まで辿れる
- 新規取り込み後に再反映される

---

## Phase 6: Incremental propagation と運用最小化（1週間）

### 作るもの
- watermark: `recordedAt + id`
- per-source watermark state
- scheduled rebuild
- incremental apply
- full rebuild 差分検証
- observer heartbeat / gap alert

### AI主導
- watermark state management
- scheduler
- rebuild diff test
- metrics / alert wiring

### 人間主導
- どの projection が incremental 向きか
- SLA / staleness policy
- drift 発見時の対処

### 完了条件
- 新規 Slack / Slides データが差分反映される
- full rebuild と比較して破綻がない
- gap alert が出る

---

## Phase 7: Governance 最小実装（1週間）

### 作るもの
- access scope: internal / restricted
- filtering projection の最小版
- audit events
- export deny / approval の雛形

### AI主導
- policy engine skeleton
- audit table / event model
- middleware
- tests

### 人間主導
- restricted の意味
- who can see what
- approval route
- export の手動運用

### 完了条件
- restricted data が素通りしない
- audit が残る
- internal MVP として安全にデモできる

---

# 6. 今は後回しにした方がよいもの

ここが超重要です。  
今の段階でやらない方がいいものを明示します。

## 後回し推奨
- source-native write-back
- generic writable projection
- full multimodal OCR/caption/embedding パイプライン
- すべてのSaaS source への一般化
- 複数 projection engine の同時運用
- DOI 自動化
- 完全な consent lifecycle
- 高度な row-level lineage 全面対応
- agent sandbox 本番運用
- GUI を最初から作り込み

MVPでは **“知識基盤の一般解” ではなく “1本の使える縦スライス”** を作るべきです。

---

# 7. AI運用ルールのおすすめ

## ルール1: 1PR 1意味
1つのPRで複数の意味変更を入れない。  
AIは広く触りがちなので、これは強いガードになります。

## ルール2: 仕様変更と実装変更を分ける
- spec PR
- code PR

を分ける。  
LETHEではこれだけで混乱がかなり減ります。

## ルール3: AIへの依頼文に必ず含めるもの
- 守るべき law
- 変更対象ファイル
- 非目標
- acceptance test
- output format

## ルール4: AI生成物は “replay test” が通るまで信用しない
特に projector / ingestion はこれが重要。

## ルール5: 高リスク変更は “AI起案・人間承認”
- policy
- secrets
- export
- delete
- write-back

は必ずこれ。

---

# 8. チーム運用のおすすめ cadence

## 週次サイクル
### 月
- 人間: 今週の仕様境界を決める
- AI: タスク分解

### 火〜木
- AI: 実装 / テスト / 修正
- 人間: lawレビュー / acceptance確認

### 金
- full rebuild / replay check
- demo
- backlog整理
- 次週の仕様確定

---

# 9. いま直近で着手すべき順番

あなたの open issues に沿うなら、順番はこうです。

1. **R2-2 Slack Schema / Adapter**
2. **R2-6 Google Slides Adapter**
3. **R2-3 名寄せ Projection**
4. **R2-4 個人ページ API**
5. **R2-1 Incremental Propagation watermark**
6. **R2-5 rebuild cost policy**
7. **R2-8 Catalog discovery**
8. **R2-7 Agent sandbox**

理由は単純で、  
**まず「縦スライスが動く」ことが先で、sandboxはその後** だからです。

---

# 10. 私ならこういう判断にします

かなり率直に言うと、LETHE では

- **アーキテクチャの最終決定は人間**
- **実装の6〜8割はAI**
- **レビューの最終責任は人間**
- **MVPは1本の垂直スライスだけ**
- **AIに広い自由を与えるのは Phase 4 以降**

が最適です。

もし一言で運用原則を置くなら、

> **AIには“速く作らせる”、人間は“何を正しいとみなすか”を握る**

です。

---

必要なら次に、
**「この仕様から実際のGitHub issue一覧を切る」**
ところまで一緒にやれます。

たとえば次の出力形式で作れます。

- Epic一覧
- 2週間スプリント計画
- 各issueのAI依頼テンプレ
- repo構成案
- MVPの受け入れテスト一覧

必要ならそのまま **実装開始可能な backlog** に落とします。