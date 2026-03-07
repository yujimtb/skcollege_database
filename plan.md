
---

# Architecture Design Specification: Dormitory Observation & Knowledge Platform (DOKP)

**Version:** 3.0
**Date:** 2026-03-03
**Context:** 学生寮データ統合基盤 & 学術的知識資産プラットフォーム

---

## 1. System Vision

### 1.1 What This System Is

学生寮に関わるあらゆる **「観測（Observation）」** を普遍的に蓄積し、そこから誰でも自由にデータベースを構築・合成できる **開放的な知識基盤**。

### 1.2 Why This System Exists

| 問題 | 本システムによる解決 |
|---|---|
| 寮内で個別に乱立するDB・スプレッドシートが統一的に扱えない | 全データが単一の Observation Lake に集約される |
| 蓄積されたデータに学術的引用可能性がない | 全 Projection に DOI 付与・再現性を保証 |
| 年度交代でデータ資産が断絶する | Lake と Registry が永続し、誰でも過去データを発見・再利用できる |

### 1.3 Design Principles

| # | Principle | Guarantees |
|---|---|---|
| P1 | **Open Entity Model** | 任意の関心対象（人、モノ、場所、概念）を自由に登録・観測できる |
| P2 | **Sovereign Projection** | 誰でも既存データから自分の DB を自由に構築できる |
| P3 | **DB on DBs** | Projection は他の Projection をソースにできる（DAG 構造で無限合成） |
| P4 | **Multimodal First** | テキスト、画像、音声、動画、センサーデータを区別なく扱える |
| P5 | **Source Agnostic** | 新しいデータソース（センサー、Bot、人間入力、外部 API）を自由に追加できる |

---

## 2. Architecture Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│                       Registry (Meta Layer)                          │
│                                                                      │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌─────────────────┐  │
│  │ EntityType │ │ Schema     │ │ Source     │ │ Projection      │  │
│  │ Registry   │ │ Registry   │ │ Registry   │ │ Catalog         │  │
│  └────────────┘ └────────────┘ └────────────┘ └─────────────────┘  │
│  ┌────────────┐ ┌────────────────────────────────────────────────┐  │
│  │ Consent    │ │ Governance Rules (Ethics, Access, Retention)   │  │
│  │ Ledger     │ │                                                │  │
│  └────────────┘ └────────────────────────────────────────────────┘  │
└────────────────────────────────┬─────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│               Observation Lake (Ground Truth)                        │
│                                                                      │
│  ┌───────────────────────────┐     ┌─────────────────────────────┐  │
│  │ Structured Observations   │────▶│ Object Storage (Blobs)      │  │
│  │ (who, what, when,         │ ref │ Images, Audio, Video,       │  │
│  │  about whom/what)         │     │ Documents, Raw Sensor Data  │  │
│  └───────────────────────────┘     └─────────────────────────────┘  │
└────────────────────────────────┬─────────────────────────────────────┘
                                 │
             ┌───────────────────┼───────────────────┐
             ▼                   ▼                   ▼
      ┌────────────┐     ┌────────────┐      ┌────────────┐
      │ Proj A     │     │ Proj B     │      │ Proj C     │
      │ (誰でも    │     │            │      │            │
      │  作成可能) │     │            │      │            │
      └─────┬──────┘     └──────┬─────┘      └────────────┘
            │                   │
            └─────────┬─────────┘
                      ▼
               ┌────────────┐
               │ Proj D     │  ← DB on DBs (A + B を合成)
               └─────┬──────┘
                     ▼
               ┌────────────┐
               │ Proj E     │  ← さらに深い合成も可能
               └────────────┘
```

固定の「L1→L2→L3」階層ではなく、**Lake を根とした DAG（有向非巡回グラフ）** として任意の深さで合成できる。Projection の「深度」は自動計算されるメタデータに過ぎず、設計上の制約にはならない。

---

## 3. Registry (Meta Layer)

### 3.1 Entity Type Registry — 「何について」観測するかの型定義

プロジェクトメンバーは誰でも新しい Entity Type を登録できる（P1: Open Entity Model）。

```yaml
# === 基盤型（システム初期登録） ===
- id: "et:person"
  name: "Person"
  description: "寮に関わる人物（寮生、スタッフ、来訪者等）"
  attributes: ["name", "affiliation", "cohort"]

- id: "et:space"
  name: "Space"
  description: "物理空間（部屋、共有スペース、建物等）"
  subtypes: ["et:room", "et:common-area", "et:dining-hall", "et:building"]
  attributes: ["building", "floor", "capacity", "purpose"]

- id: "et:artifact"
  name: "Artifact"
  description: "物理的・デジタルな対象物"
  subtypes: ["et:book", "et:equipment", "et:document", "et:furniture"]
  attributes: ["name", "category"]

# === 利用者が自由に追加する型の例 ===
- id: "et:meal-event"
  name: "Meal Event"
  description: "食堂での食事イベント"
  registered_by: "tanaka"
  registered_at: "2026-05-15"

- id: "et:study-group"
  name: "Study Group"
  description: "自主的に形成された学習グループ"
  registered_by: "yamamoto"
  registered_at: "2026-06-01"
```

**型の階層（is-a）:** `et:room` is-a `et:space` のように継承関係を持てる。Projection は親型でフィルタすることで配下の全サブタイプを取得できる。

### 3.2 Schema Registry — 「どのような形式で」観測するかの定義

各 Observation は必ずいずれかの Schema に適合しなければならない。Schema は JSON Schema で記述される。

```yaml
- id: "schema:room-entry"
  name: "Room Entry/Exit"
  version: "1.0.0"
  subject_type: "et:person"       # 誰が
  target_type: "et:room"          # どこに
  payload_schema:
    type: object
    properties:
      action: { enum: ["enter", "exit"] }
      method: { enum: ["key-card", "manual", "auto-sensor"] }
    required: ["action"]

- id: "schema:temperature-reading"
  name: "Temperature Reading"
  version: "1.0.0"
  subject_type: "et:space"        # どこの
  payload_schema:
    type: object
    properties:
      celsius: { type: number }
      humidity_pct: { type: number }
    required: ["celsius"]

- id: "schema:photo-record"
  name: "Photo Record"
  version: "1.0.0"
  subject_type: "et:*"            # 任意のエンティティについて
  payload_schema:
    type: object
    properties:
      description: { type: string }
      labels: { type: array, items: { type: string } }
    required: ["description"]
  attachments:
    required: true
    accepted_types: ["image/jpeg", "image/png", "image/webp"]

- id: "schema:freeform-note"
  name: "Freeform Note"
  version: "1.0.0"
  subject_type: "et:*"
  payload_schema:
    type: object
    properties:
      text: { type: string }
      tags: { type: array, items: { type: string } }
    required: ["text"]

- id: "schema:document-snapshot"
  name: "Document Snapshot"
  version: "1.0.0"
  subject_type: "et:document"
  payload_schema:
    type: object
    properties:
      provider: { enum: ["google-slides", "google-sheets", "spreadsheet", "other"] }
      document_type: { enum: ["slide-deck", "spreadsheet", "document", "other"] }
      source_document_id: { type: string }
      source_revision_id: { type: string }
      source_modified_at: { type: string, format: date-time }
      capture_mode: { enum: ["native-structure", "rendered-snapshot", "hybrid"] }
      native_structure_ref: { type: string }
      render_refs:
        type: array
        items: { type: string }
    required: ["provider", "document_type", "source_document_id", "capture_mode"]
  attachments:
    required: false
    accepted_types: ["application/json", "application/pdf", "application/vnd.openxmlformats-officedocument.presentationml.presentation", "image/png"]
```

**拡張の自由度:** 新しい Schema は誰でも登録できる。`subject_type: "et:*"` を指定すればあらゆるエンティティに対して使える汎用スキーマとなる。

### 3.3 Source Registry — 「何が」データを送り込むか

```yaml
- id: "src:door-sensor-bldg-a"
  name: "Building A Door Sensors"
  type: "sensor"
  schemas: ["schema:room-entry"]
  owner: "facilities-team"
  trust_level: "automated"        # automated | human-verified | crowdsourced

- id: "src:dining-bot"
  name: "Dining Hall Usage Bot"
  type: "bot"
  schemas: ["schema:dining-entry", "schema:dining-exit"]
  owner: "infra-team"
  trust_level: "automated"

- id: "src:manual-tanaka"
  name: "Tanaka Manual Entry"
  type: "human"
  schemas: ["*"]                  # 任意の Schema で投稿可能
  owner: "tanaka"
  trust_level: "human-verified"

- id: "src:weather-api"
  name: "External Weather API"
  type: "external-api"
  schemas: ["schema:weather-reading"]
  owner: "infra-team"
  trust_level: "automated"

- id: "src:gslides-crawler"
  name: "Google Slides Crawler"
  type: "external-api"
  schemas: ["schema:document-snapshot"]
  owner: "infra-team"
  trust_level: "automated"
```

**新規ソースの追加（P5）:** Source を Registry に登録し、認証トークンを取得すれば、即座にデータ投入を開始できる。

**Mutable な外部ソース:** Google Slides やスプレッドシートのような mutable source は、現在値を上書き保存するのではなく、各 revision/timepoint を表す **Snapshot Observation** として Lake に append する。

### 3.4 Projection Catalog — 構築された DB の発見・利用

```yaml
- id: "proj:social-graph-2026s"
  name: "Social Co-presence Graph (Spring 2026)"
  description: |
    共有スペースの入退室データから同時滞在ネットワークを構築。
    ノード＝Person、エッジ＝同一空間への同時滞在（15分以上）。
  created_by: "yamamoto"
  created_at: "2026-05-01"
  version: "1.2.0"
  engine: "neo4j"
  sources:
    - type: "lake"
      filter:
        schemas: ["schema:room-entry"]
        window: { start: "2026-04-01", end: "2026-09-30" }
  connection:
    protocol: "bolt"
    endpoint: "bolt://192.168.x.x:7687"
  outputs:
    - format: "cypher"
      description: "Native graph queries"
    - format: "parquet"
      description: "Weekly snapshot exports"
      location: "/exports/social-graph/2026s/"
  doi: "10.xxxxx/dokp-sg-2026s"   # 学術引用用 DOI
  tags: ["network", "social", "co-presence"]
  status: "active"                 # active | archived | building
```

---

## 4. Observation Lake (Ground Truth)

### 4.1 Core Concept

システムの根幹となる不変の事実ストア。全てのデータは **Observation（観測）** という統一的な単位で記録される。

> **Observation =「ある情報源が、あるエンティティについて、何かを観測した」**

### 4.2 Observation Schema

```
┌──────────────────────────────────────────────────────────────┐
│                        Observation                            │
├──────────────────────────────────────────────────────────────┤
│ id             : UUID v7 (time-sortable)                     │
│ schema         : SchemaRef   → Schema Registry               │
│ schemaVersion  : SemVer      (e.g. "1.0.0")                 │
│                                                              │
│ source         : SourceRef   → Source Registry               │
│ actor          : EntityRef?  (human who triggered, if any)   │
│                                                              │
│ subject        : EntityRef   (PRIMARY: what this is about)   │
│ target         : EntityRef?  (SECONDARY: related entity)     │
│                                                              │
│ payload        : JSON        (validated against schema)      │
│ attachments    : [BlobRef]   (→ Object Storage)              │
│                                                              │
│ published      : ISO 8601    (claimed event time)            │
│ recordedAt     : ISO 8601    (system ingestion time, auto)   │
│                                                              │
│ consent        : ConsentRef  → Consent Ledger                │
│ idempotencyKey : string      (deduplication key)             │
│ meta           : JSON        (freeform operational metadata) │
└──────────────────────────────────────────────────────────────┘
```

**EntityRef format:** `{type}:{id}` — 例: `person:tanaka-2026`, `room:A-301`, `meal-event:2026-05-01-lunch`

### 4.3 Concrete Examples

```jsonc
// Example 1: Door sensor records a room entry
{
  "id": "019577a0-...",
  "schema": "schema:room-entry",
  "schemaVersion": "1.0.0",
  "source": "src:door-sensor-bldg-a",
  "subject": "person:tanaka-2026",
  "target": "room:A-301",
  "payload": { "action": "enter", "method": "key-card" },
  "attachments": [],
  "published": "2026-05-01T08:30:00+09:00",
  "recordedAt": "2026-05-01T08:30:00.123+09:00",
  "consent": "consent:tanaka-2026-main",
  "idempotencyKey": "door-a301-tanaka-20260501-0830",
  "meta": { "sensor_firmware": "v2.1" }
}

// Example 2: Manual photo record of whiteboard
{
  "id": "019577b2-...",
  "schema": "schema:photo-record",
  "schemaVersion": "1.0.0",
  "source": "src:manual-suzuki",
  "actor": "person:suzuki-2026",
  "subject": "room:common-B2",
  "payload": {
    "description": "ミーティング後のホワイトボード",
    "labels": ["meeting", "project-alpha"]
  },
  "attachments": ["blob:sha256:a1b2c3d4..."],
  "published": "2026-05-01T14:23:00+09:00",
  "recordedAt": "2026-05-01T14:23:05.456+09:00",
  "consent": "consent:suzuki-2026-main",
  "idempotencyKey": "suzuki-photo-20260501-1423",
  "meta": {}
}

// Example 3: External API weather data
{
  "id": "019577c3-...",
  "schema": "schema:weather-reading",
  "schemaVersion": "1.0.0",
  "source": "src:weather-api",
  "subject": "space:campus",
  "payload": { "celsius": 22.5, "humidity_pct": 65, "condition": "cloudy" },
  "attachments": [],
  "published": "2026-05-01T12:00:00+09:00",
  "recordedAt": "2026-05-01T12:00:02.789+09:00",
  "idempotencyKey": "weather-campus-20260501-1200",
  "meta": { "api_provider": "openweathermap" }
}
```

### 4.4 Storage Architecture

| Data Type | Storage | Format | Retention |
|---|---|---|---|
| Structured Observations | Event Store (append-only log) | JSON / Avro | **永続** |
| Binary Attachments | Object Storage (MinIO / S3互換) | Original format, CAS hash | ポリシーベース |
| High-Frequency Raw Time-Series | Object Storage / TSDB / Data Lakehouse | Parquet / Zarr / Arrow / native TSDB blocks | ポリシーベース |
| Cold Archive | Partitioned files | Parquet (per schema, per month) | **永続** |

**Content-Addressable Storage (CAS):** バイナリデータは SHA-256 ハッシュをキーとして保存される。同一ファイルは自動的に重複排除される。参照形式: `blob:sha256:<hash>`

高頻度な生時系列は、必ずしも 1 サンプル = 1 Observation として Lake に格納する必要はない。Lake の役割はあくまで canonical な発見性・参照性・再現性の確保であり、大容量の連続系列本体は外部ストアに置き、その存在と意味を Observation として登録してよい。

### 4.5 Ingestion Pipeline

```
Source
  │
  ▼
Ingestion Gate ──────────────────────────────────────
  │
  ├─ 1. Authenticate source     (Source Registry)
  ├─ 2. Validate payload         (Schema Registry: JSON Schema check)
  ├─ 3. Check consent status     (Consent Ledger: subject に同意があるか)
  ├─ 4. Deduplicate              (idempotencyKey で重複検知)
  ├─ 5. Store blobs → get BlobRefs  (Object Storage)
  ├─ 6. Assign recordedAt        (system timestamp)
  ├─ 7. Assign id                (UUID v7)
  │
  ▼
Observation Lake (append)
```

### 4.5.1 Mutable External Artifact Ingestion

Google Slides、スプレッドシート、共同編集ドキュメントのような mutable artifact は、イベント差分ではなく **revisioned snapshot source** として扱う。

取り込み時の原則:

1. **Native Snapshot を取得**
   Source API が返す構造情報をそのまま取得する。
   例: Google Slides の page/pageElement/objectId/text/speaker notes/layout/master 情報。

2. **Rendered Snapshot を取得**
   人間が見ている見た目を保存するため、同一 revision の PDF、PNG、PPTX 等を取得する。

3. **Hybrid Observation として append**
   1 と 2 を同一 source revision に紐づく 1 つの Observation として Lake に保存する。

4. **Semantic Enrichment は行わない**
   OCR、captioning、embedding、要約、分類などの意味付けは Ingestion では実施しない。これらは downstream Projection として扱う。

この原則により、Lake には「解釈前の一次資料」のみが入り、後段の multimodal/LLM 処理を何度でも再実行できる。

```jsonc
{
  "schema": "schema:document-snapshot",
  "source": "src:gslides-crawler",
  "subject": "document:gslide:deck-abc123",
  "payload": {
    "provider": "google-slides",
    "document_type": "slide-deck",
    "source_document_id": "deck-abc123",
    "source_revision_id": "rev-017",
    "source_modified_at": "2026-03-07T09:10:00+09:00",
    "capture_mode": "hybrid",
    "native_structure_ref": "blob:sha256:native-json...",
    "render_refs": [
      "blob:sha256:deck-pdf...",
      "blob:sha256:slide-1-png...",
      "blob:sha256:slide-2-png..."
    ]
  },
  "attachments": [
    "blob:sha256:native-json...",
    "blob:sha256:deck-pdf...",
    "blob:sha256:slide-1-png...",
    "blob:sha256:slide-2-png..."
  ],
  "published": "2026-03-07T09:10:00+09:00",
  "idempotencyKey": "gslides:deck-abc123:rev-017"
}
```

  ### 4.5.2 Sensor Time-Series Ingestion

  センサーの時系列データは、"mutable な 1 行" としてではなく、時間順に蓄積される **observation stream** として扱う。

  ただし、全てのセンサーデータを同じ粒度で Observation 化する必要はない。更新頻度・意味密度・再利用性に応じて、Lake と外部 raw store を使い分ける。

  基本方針:

  1. **Meaningful Event は Observation として append する**
    ドア入退室、状態遷移、アラート発火、装置交換、較正変更のように、意味的な境界を持つ出来事は 1 Observation ずつ追加する。

  2. **Stateful Sensor は transition または interval として表現する**
    ドア開閉、在席、点灯状態のように状態遷移を持つものは、必要に応じて `open -> close` の transition event、または `started_at / ended_at` を持つ区間 Observation として表す。

  3. **High-Frequency Raw Samples は chunked raw store に置く**
    サンプリング周波数が高く、1 点ごとの意味が薄い連続系列は、Object Storage / TSDB / Lakehouse にチャンク単位で保存し、Lake にはその chunk を指す manifest Observation だけを置く。

  4. **Current Value は canonical data にしない**
    "最新温度" "現在の在室状態" のような見かけ上 mutable な値は、Lake 内の正史ではなく Projection で導出する。

  5. **Late Arrival / Duplicate / Re-send を前提にする**
    センサーは遅延送信、再送、順不同到着を起こしうるため、`published` を event time、`recordedAt` を ingestion time として分離し、`idempotencyKey` で重複排除する。

  6. **補正は再書き込みではなく correction Observation で表現する**
    校正ミス、単位変換ミス、異常値除外、欠損補完は元 Observation を直接更新せず、参照付きの correction / retraction Observation で表現する。

  7. **デバイス設定変更は別 Observation として保存する**
    センサー交換、設置位置変更、較正係数更新、firmware 更新は、読み値そのものとは別の operational Observation として保持する。

  canonical と derived の切り分け:

  - canonical in Lake: 意味的イベント、状態遷移、区間、raw chunk manifest、装置メタデータ変更
  - canonical outside Lake body: 高頻度 raw samples 本体
  - derived: latest value、1分/1時間ロールアップ、異常検知、補間系列、欠損補完系列

  切り分けの目安:

  - 低頻度かつ意味的に独立した測定: 1 sample = 1 Observation
  - 高頻度だが研究再現のため raw 保持が必要: raw chunk を外部保存し、chunk manifest を Observation 化
  - 単なる運用監視で生波形の永続保持が不要: rollup のみ canonical 化し、raw は短期保持または破棄

  この規則により、Lake は常に再計算可能な一次データの索引と意味的境界を保持し、運用上必要な "現在値" や "整形済み系列" は Projection として自由に作り直せる。

  ```jsonc
  {
    "schema": "schema:raw-timeseries-chunk",
    "source": "src:env-sensor-a301",
    "subject": "space:room:A-301",
    "payload": {
     "series_type": "environment",
     "sensor_id": "env-a301",
     "time_range": {
      "start": "2026-03-07T09:00:00+09:00",
      "end": "2026-03-07T09:00:59.999+09:00"
     },
     "sample_count": 600,
     "sampling_hz": 10,
     "encoding": "parquet",
     "raw_ref": "blob:sha256:env-a301-20260307T0900-parquet...",
     "channels": ["celsius", "humidity_pct"]
    },
    "attachments": ["blob:sha256:env-a301-20260307T0900-parquet..."],
    "published": "2026-03-07T09:00:59.999+09:00",
    "recordedAt": "2026-03-07T09:00:01.245+09:00",
    "idempotencyKey": "env-a301-20260307T0900-chunk-0001",
    "meta": {
     "sequence_start": 482391,
     "sequence_end": 482990,
     "firmware": "v1.8.2"
    }
  }
  ```

  最新状態が必要な場合は、例えば次のような Projection を構築する。

  - room-latest-environment: 各部屋について最新 reading を 1 行に射影
  - room-environment-rollup-1m: 1 分窓で平均・最小・最大を集計
  - occupancy-intervals: 入退室イベント列から在室区間を再構成

  分析時には、Projection engine が manifest Observation を辿って raw chunk を読み込めればよい。これにより、Lake は軽量な control plane、外部 raw store は重量級 data plane として役割分担できる。

### 4.6 Immutability & Corrections

Lake は **Append-Only** であり、過去の Observation を変更・削除しない。訂正は新しい Observation で表現する。

| Intent | Mechanism |
|---|---|
| 新規データの記録 | 新しい Observation |
| 過去の誤りを訂正 | 新しい Observation + `meta.corrects: "<original-id>"` |
| データの撤回 | 新しい Observation + `meta.retracts: "<original-id>"` |
| オプトアウト | Consent Ledger にフラグ → Projection 構築時に反映 |

### 4.7 Event Ordering (Determinism)

Observation の適用順序（Projection が再生する際の順序）：

1. `published` (Event Time — 事象の発生時刻)
2. `recordedAt` (System Time — 記録時刻)
3. `id` (UUID v7 — 時刻ソート可能な ID のバイト順)

---

## 5. Projection Ecosystem

### 5.1 What is a Projection?

Projection は、Observation Lake または他の Projection をソースとして **変換ロジック** を適用し構築された **派生データベース**。

- **Owned:** 個人またはチームが作成・管理する
- **Registered:** Projection Catalog に登録され、誰でも発見できる
- **Reproducible:** ソース宣言 + 変換ロジックから再構築可能
- **Disposable:** いつでも破棄・再構築できる（Ground Truth は Lake にある）
- **Composable:** 他の Projection のソースになれる（P3: DB on DBs）

### 5.2 Source Declaration & DAG

```
                  Observation Lake
                   /    |    \
                  /     |     \
          Proj:A    Proj:B    Proj:C    ← depth 1 (Lake のみをソース)
            |         |
            +----+----+
                 |
              Proj:D                    ← depth 2 (A + B を合成: DB on DBs)
                 |
              Proj:E                    ← depth 3 (D をさらに加工)
```

- **depth 1:** Lake のみをソースとする Projection
- **depth N:** 少なくとも1つの depth N-1 の Projection をソースに含む
- **非巡回性の強制:** 循環依存は Catalog 登録時にシステムが拒否する

### 5.3 Projection Definition File

各 Projection は宣言的な Spec ファイルで定義される。

```yaml
apiVersion: "dokp/v1"
kind: "Projection"

metadata:
  id: "proj:dining-patterns-2026"
  name: "Dining Pattern Analysis"
  created_by: "suzuki"
  version: "1.0.0"
  tags: ["dining", "behavioral", "timeseries"]
  description: |
    食堂利用パターンの分析。入退出観測を集計し、
    時間帯別利用率と個人別利用頻度プロファイルを生成。

spec:
  # ── ソース宣言 ──
  sources:
    - ref: "lake"
      filter:
        schemas: ["schema:dining-entry", "schema:dining-exit"]
        subject_types: ["et:person"]
        window: { start: "2026-04-01", end: "2026-09-30" }
    - ref: "proj:person-directory-2026"   # ← DB on DBs: 別の Projection を利用
      version: ">=1.0.0"

  # ── 構築設定 ──
  engine: "duckdb"
  build:
    type: "sql-migration"
    entrypoint: "./projections/dining/migrate.sql"
    projector: "./projections/dining/projector.py"

  # ── 出力定義 ──
  outputs:
    - format: "sql"
      tables: ["hourly_occupancy", "person_frequency", "peak_analysis"]
    - format: "parquet"
      schedule: "weekly"
      location: "/exports/dining-patterns/"

  # ── 再現性 ──
  reproducibility:
    deterministic: true
    seed: 42
    rebuild_command: "make rebuild-dining"
```

### 5.4 Projection Lifecycle

```
1. Define    → Projection Spec を記述（YAML + 変換コード）
2. Register  → Projection Catalog に登録（DAG の非巡回性チェック）
3. Build     → 変換を実行、対象 DB を構築
4. Serve     → クエリ可能な状態。他の Projection のソースにもなれる
5. Version   → Spec 更新 → バージョンアップ → 再ビルド
6. Archive   → 非活性化、最終スナップショットをエクスポート、DOI を凍結
```

### 5.5 Typical Projection Patterns

| Pattern | Example | Engine |
|---|---|---|
| **Social Network** | 同時滞在グラフ（入退室データから） | Neo4j / NetworkX |
| **Time Series** | センサーデータ集計、利用率推移 | TimescaleDB / DuckDB |
| **Behavioral Embedding** | 行動パターンのベクトル表現、クラスタリング | pgvector / FAISS |
| **Text Corpus** | ノート、コメント、アンケート回答の全文検索 | Elasticsearch / SQLite FTS |
| **Multimedia Index** | 画像メタデータ + 視覚的埋め込みベクトル | pgvector + MinIO |
| **Operational View** | 現在の部屋割り、設備稼働状況 | PostgreSQL |
| **Composite Analysis** | ソーシャル × 食生活 × 学業のクロス分析 | DuckDB / Jupyter + Parquet |

### 5.6 Access Patterns

Projection を利用する際の手段：

| Pattern | Description | Use Case |
|---|---|---|
| **Native Query** | Manifest の `connection` で直接クエリ（SQL, Cypher 等） | リアルタイム分析 |
| **Bulk Export** | Parquet / CSV スナップショットをダウンロード | 大規模バッチ処理 |
| **Source Reference** | 自分の Projection Spec の `sources` に宣言 | DB on DBs 合成 |

---

## 6. Multimodal Support

### 6.1 Architecture

```
Observation Event (structured, in Lake)
    │
    ├── payload: { "description": "Meeting whiteboard", "labels": ["project-x"] }
    │
    └── attachments:
         ├── blob:sha256:a1b2c3... → image/jpeg  (3.2MB, Object Storage)
         └── blob:sha256:d4e5f6... → audio/wav   (12MB,  Object Storage)
```

Observation 自体は常に構造化メタデータであり、バイナリデータは Object Storage に格納されて BlobRef で参照される。

### 6.2 Blob Metadata

```json
{
  "hash": "sha256:a1b2c3d4e5f6...",
  "mime_type": "image/jpeg",
  "size_bytes": 3355443,
  "original_filename": "whiteboard_2026-05-01.jpg",
  "uploaded_at": "2026-05-01T14:23:00Z",
  "uploaded_by": "src:manual-suzuki",
  "dimensions": { "width": 4032, "height": 3024 }
}
```

### 6.3 Downstream Processing

マルチモーダルデータの加工は Projection として実装する。特別な仕組みは不要。

| Projection Type | Processing | Output |
|---|---|---|
| Image Projection | OCR / CLIP で埋め込みベクトル生成 | Text + Vector DB |
| Audio Projection | 音声認識（Whisper 等）で文字起こし | Text corpus |
| Video Projection | キーフレーム抽出、活動検出 | Metadata + Image refs |
| Sensor Fusion | 複数センサーデータの統合・補間 | Time series |

### 6.4 Mutable Multimodal Documents

Google Slides のような mutable かつ multimodal な source は、次の 2 層を canonical data として扱う。

| Layer | Contains | Role |
|---|---|---|
| **Native Canonical** | Source API の構造情報（object graph, text, notes, layout, ids） | 逆変換・差分検出・構造検索の基盤 |
| **Render Canonical** | PNG/PDF/PPTX などの視覚スナップショット | 人間視点の保存、multimodal LLM 入力 |

この 2 層は同じ source revision に属する一次資料であり、どちらも Lake に保存してよい。一方、以下は canonical data に含めず Projection として扱う。

- OCR 結果
- Image caption
- Embedding / vector
- LLM 要約
- LLM によるレイアウト解釈や意味ラベル

したがって、multimodal LLM は canonical render snapshot を読んでよいが、その出力は canonical source ではなく派生データである。

---

## 7. Governance & Ethics

### 7.1 Consent Ledger

人物（`et:person`）を subject とする Observation は、対象者の同意記録が Consent Ledger に存在しなければ Lake に投入できない。

```yaml
consent:
  id: "consent:tanaka-2026-main"
  subject: "person:tanaka-2026"
  granted_at: "2026-04-01T09:00:00+09:00"
  scope:
    - "schema:room-entry"           # 入退室センサーデータに同意
    - "schema:dining-entry"         # 食堂利用データに同意
    # "schema:survey-response" は未記載 → 収集不可
  opt_out_policy: "anonymize"       # anonymize | drop | pseudonymize
  review_date: "2027-03-31"        # 同意見直し期日
  irb_reference: "IRB-2026-042"    # 倫理審査承認番号
```

### 7.2 Opt-Out Handling

同意撤回時、Projection は以下のいずれかの戦略で処理する（`opt_out_policy` に従う）。

| Strategy | Behavior in Projections |
|---|---|
| **Drop** | 当該 subject の全 Observation を完全除外 |
| **Anonymize** | Observation は含めるが、subject を不可逆ハッシュに置換 |
| **Pseudonymize** | subject を可逆的仮名に置換（復号鍵は倫理委員会が保管） |

### 7.3 Access Control

| Role | Lake Write | Lake Read | Projection Access | Registry |
|---|---|---|---|---|
| **System Admin** | via Source | Full | All | Full CRUD |
| **Researcher** | via registered Source | Filtered by consent | Own + shared | Read + Register |
| **Resident** | via approved Source | Own data only | Approved only | Read |
| **External** | ✗ | ✗ | Published exports only | Read Catalog |

### 7.4 Data Retention

| Data | Default Retention | Override |
|---|---|---|
| Lake Observations | 永続 | IRB 条件に従い短縮可能 |
| Binary Attachments | 5年 | Projection が参照中は延長 |
| Projection Snapshots | Projection の Archive まで | DOI 付与済みは永続保存 |
| Consent Records | 永続 | 法的要件に従う |

---

## 8. Schema Evolution

### 8.1 Versioning Rules

| Change Type | Allowed? | Action |
|---|---|---|
| Optional フィールド追加 | ✓ | Minor version bump (e.g. 1.0 → 1.1) |
| Required フィールド追加 | 新バージョン | Major version bump (e.g. 1.x → 2.0) |
| フィールド削除 | 新バージョン | Major version bump |
| 型変更 | 新バージョン | Major version bump |

過去の Observation は書き込み時の schemaVersion を永久に保持する。

### 8.2 Projector Compatibility

Projection は対応する Schema バージョン範囲を宣言する。

```yaml
sources:
  - ref: "lake"
    filter:
      schemas:
        - name: "schema:room-entry"
          versions: ">=1.0.0, <3.0.0"   # v1.x と v2.x に対応
```

破壊的変更（Major bump）後も、既存 Projection は旧データで動作し続ける。新形式のデータを取り込むには Projection 側の更新が必要。

---

## 9. Academic Integrity

### 9.1 Reproducibility

同一ソース + 同一 Projection Spec + 同一 seed から、同一の結果が再構築できること。

- Projection Spec（YAML + 変換コード）は Git 管理
- ソースとなる Lake の Time Window は Spec 内に明記
- 非決定的要素（ランダムシード、外部 API 呼び出し等）は Spec に記録

### 9.2 Citability

- 全 Projection に **DOI** (Digital Object Identifier) を付与可能
- Projection Catalog が学術的 provenance record を兼ねる
- 引用例: `Yamamoto, S. (2026). Social Co-presence Graph, Spring 2026. DOKP. doi:10.xxxxx/dokp-sg-2026s`

### 9.3 Lineage Tracking

任意の Projection の出力レコードから、原始 Observation まで辿れるようにする。

```
Proj:E の結果行 → Proj:D の結果行 → Proj:A/B の結果行 → Lake の Observation → Source
```

これは Projection Spec の `sources` 宣言と Lake の `id` チェーンで実現される。

---

## 10. Practical Workflows

### 10.1 「Xについて研究したい」（新しい研究テーマの立ち上げ）

```
1. Registry を確認:
   → X に対応する EntityType は存在するか？
     → Yes: ステップ 3 へ
     → No:  新しい EntityType を登録

2. Registry を確認:
   → X を観測するための Schema は存在するか？
     → Yes: ステップ 3 へ
     → No:  新しい Schema を定義・登録

3. データ収集を開始:
   → Source を登録（センサー / Bot / 手動フォーム / API）
   → Observation を Lake に投入開始

4. Projection を構築:
   → Projection Spec を記述
   → 変換ロジックを実装
   → Projection Catalog に登録
   → Build & Serve

5. (Optional) 既存 Projection と合成:
   → Catalog で関連 Projection を発見
   → DB on DBs で合成 Projection を構築
```

### 10.2 「誰かのデータを使いたい」

```
1. Projection Catalog を閲覧
2. 対象 Projection の description / schema docs / DOI を確認
3. アクセス手段を選択:
   a. Native Query（エンジンに直接接続）
   b. Bulk Export（Parquet / CSV スナップショット）
   c. 自分の Projection の source として宣言（DB on DBs）
4. 論文等で引用する場合は DOI を使用
```

### 10.3 年度を跨いだ継続運用

```
Year N:
  - Lake は継続的に成長
  - 新しい Schema / EntityType / Source が自由に追加される
  - Projection が作られ、研究に利用される

Year N → N+1 引き継ぎ:
  - 前年チームが自身の Projection を Archive（最終スナップショット + DOI 凍結）
  - 次年チームが引き継ぐもの:
      ・ Lake（過去の全 Observation）
      ・ Registry（全ての型 / スキーマ / ソース定義）
      ・ Projection Catalog（過去の全 Projection、active / archived）
  - 次年チームができること:
      ・ 過去の任意の Projection をソースから再構築
      ・ 過去データ＋新データで新しい Projection を構築
      ・ Schema / EntityType を自由に拡張
```

---

## 11. Technology Recommendations

### 11.1 Full Stack (推奨構成)

| Component | Technology | Rationale |
|---|---|---|
| Lake (Event Store) | Apache Kafka → Parquet on MinIO | ストリーミング取り込み + クエリ可能なコールドストレージ |
| Object Storage | MinIO (S3 互換) | セルフホスト、マルチモーダル Blob 保存 |
| Schema Registry | Confluent Schema Registry | Avro/JSON Schema の検証・互換性管理 |
| Registry DB | PostgreSQL | 構造化メタデータストア |
| Projection Engines | 用途別に選定 | Neo4j / TimescaleDB / DuckDB / pgvector 等 |
| Projection Catalog | PostgreSQL + Web UI | 発見・ドキュメント・DAG 可視化 |
| Version Control | Git | Projection Spec / Schema / 変換コード |
| Auth | Keycloak / OAuth2 | Source 認証・ユーザー認可 |

### 11.2 Minimal Viable Stack（学生チーム向け最小構成）

```
┌──────────────────────────────────────────────────────────┐
│  SQLite          → Registry + Lake metadata              │
│  Parquet files   → Lake cold storage (per schema/month)  │
│  Local FS / NAS  → Object Storage (Blobs)                │
│  Git repo        → Schema 定義, Projection Spec, 変換コード│
│  DuckDB          → Default Projection engine             │
│  Python scripts  → Ingestion Gate + Projector            │
└──────────────────────────────────────────────────────────┘
```

**単一マシンで全アーキテクチャを実装可能。** 規模拡大に応じてコンポーネントをスケールアウトする。

### 11.3 Migration Path

```
Phase 1 (MVP):     SQLite + Parquet + DuckDB + Git
Phase 2 (Growth):  PostgreSQL + MinIO + Kafka Connect
Phase 3 (Scale):   Full Stack (Kafka + MinIO + Keycloak + Neo4j + ...)
```

---

## 12. CQRS: Write-Back via Lake

Projection 上の UI やアプリケーションからの変更は、**Projection に直接書き込まない**。必ず新しい Observation として Lake に投入し、Projector 経由で反映させる。

```
UI で「部屋割り変更」操作
  │
  ▼
Command: Observation { schema: "schema:room-assignment", ... }
  │
  ▼
Ingestion Gate → Lake (append)
  │
  ▼
Projector (room-management) が検知 → Projection を更新
```

これにより、全ての変更が Lake に記録され、監査可能性・再現性が保たれる。

---

## 13. Functional Projection Model & Writable Views

### 13.1 Functional Interpretation

本システムは実装上は DB 群から構成されるが、設計上は以下の関数型的モデルとして解釈する。

> **Projection = ordered Observations を入力として受け取り、決定的な状態または出力を返す純粋な導出関数**

概念的には次の fold として表現できる。

```text
ProjectionResult = finalize(foldl(apply, initialState, orderedObservations))
```

ここで:

- `orderedObservations` は 4.7 で定義した順序規則に従う
- `apply` は Observation を状態へ適用する純粋関数
- `finalize` は内部状態から外部公開用の表・グラフ・ベクトル等を生成する純粋関数

このモデルにおいて、副作用は Projection 本体ではなく以下に隔離される。

- Source 認証
- Blob 保存
- `recordedAt` 付与
- UUID v7 採番
- 実 DB / ファイルへの materialization

### 13.2 Projection Mutability Rules

Projection は原則として **read-only** である。利用者が「Projection を編集している」と感じる操作が存在しても、それは内部的には Projection への直接更新ではなく、Lake に対する新規 Observation の追加として扱う。

このため、Projection の可変性は次の 3 層に分ける。

| Layer | Meaning | Allowed Write Path |
|---|---|---|
| **Lake** | 唯一の Ground Truth | Observation append only |
| **Projection Materialization** | Lake / 他 Projection の計算結果 | 直接更新禁止 |
| **UI View / Draft Workspace** | 利用者が編集しているように見える作業面 | Command 発行のみ |

### 13.3 Writable Projection

一部の Projection は、UI 上で insert / update / delete を受け付けてもよい。ただし、その Projection は **Writable Projection** として明示的に宣言されなければならない。

Writable Projection の必須条件:

1. 操作が Lake に対する Observation 列へ逆変換できること
2. 逆変換規則が Projection Spec に宣言されていること
3. Replay 後に同じ結果へ収束すること
4. 追加・修正の provenance が保存されること

Writable Projection であっても、materialized table / graph / index への直接 insert/update/delete は禁止する。

### 13.4 Write Adapter (Inverse Mapping)

Writable Projection は **Write Adapter** を持つ。Write Adapter は、Projection 上の操作を Lake に追加すべき Command / Observation 群へ変換する逆写像である。

```text
UI Insert / Update / Delete
  -> Write Adapter
  -> Command
  -> Observation append to Lake
  -> Projector replay / incremental apply
  -> Projection updated
```

Write Adapter は関数型プログラミングでいう lens / prism 的な役割を持つが、本仕様では以下の制約を持つ。

- Projection の全行が writable である必要はない
- 逆変換できない操作は拒否または proposal 化される
- Write Adapter は hidden mutable state を持ってはならない
- 変換結果は deterministic でなければならない

### 13.5 Write Modes

Projection への見かけ上の書き込みは、必ず次のいずれかの mode に分類される。

| Mode | Meaning | Stored in Lake as |
|---|---|---|
| **canonical** | 正規のドメイン事実を追加・修正・撤回する | Canonical Observation |
| **annotation** | 派生結果への注釈・ラベル・レビュー等を付与する | Annotation Observation |
| **proposal** | 正規事実へ即時変換できない追加・修正案を保持する | Proposal Observation |

#### canonical

Projection 上の操作が、正規の事実として一意に解釈できる場合に使う。

例:

- 現在の部屋割り Projection に新しい割当を追加する
- 人物 directory Projection に所属変更を反映する

#### annotation

Projection 行そのものを Ground Truth にしたいのではなく、その派生結果に対する人間の知見を付与したい場合に使う。

例:

- 合成分析結果に「要確認」「異常値候補」ラベルを付ける
- グラフ上のノードに研究者注記を追加する

#### proposal

操作の意図は有益だが、1 回の insert から正規 Observation を確定できない場合に使う。

例:

- 合成済みの分析表に新しい 1 行を追加したいが、元の subject / target / event type が一意に決まらない
- 外部共同研究者が補助情報を追加したが、寮内の正規 Schema へまだ対応していない

Proposal Observation は承認後に canonical Observation へ変換されてもよい。

### 13.6 Projection Spec Extension for Write-Back

Writable Projection は、既存の Projection Spec を次のように拡張して宣言できる。

```yaml
apiVersion: "dokp/v1"
kind: "Projection"

metadata:
  id: "proj:room-assignment-view"
  name: "Room Assignment View"
  version: "1.0.0"

spec:
  sources:
    - ref: "lake"
      filter:
        schemas: ["schema:room-assignment", "schema:room-assignment-change"]

  engine: "duckdb"
  build:
    type: "sql-migration"
    projector: "./projections/room_assignment/projector.py"

  reproducibility:
    deterministic: true

  writeBack:
    enabled: true
    mode: "canonical"
    acceptedCommands:
      - "assign-room"
      - "change-room-assignment"
      - "vacate-room"
    inverseMapping:
      adapter: "./projections/room_assignment/write_adapter.py"
    reviewPolicy:
      required: false
    lineage:
      captureProjectionContext: true
      captureVisibleRowHash: true
```

複合分析 Projection の例:

```yaml
writeBack:
  enabled: true
  mode: "annotation"
  acceptedCommands:
    - "tag-derived-record"
    - "attach-review-note"
  inverseMapping:
    adapter: "./projections/composite_analysis/write_adapter.py"
  reviewPolicy:
    required: false
```

逆に、正規事実へ戻せない Projection は次のように明示的に read-only とする。

```yaml
writeBack:
  enabled: false
  reason: "derived metrics cannot be losslessly inverted into canonical observations"
```

### 13.7 Command and Observation Requirements

Write Adapter が生成する Command / Observation には、少なくとも以下を含めなければならない。

- `source`: どの UI / user / automation が操作したか
- `actor`: 操作主体
- `schema`: 生成対象の Schema
- `published`: 利用者が主張する事象時刻、または操作時刻
- `idempotencyKey`: 重複追加防止用キー
- `meta.projectionContext.projectionId`: どの Projection 上で操作したか
- `meta.projectionContext.visibleRowHash`: 利用者が見ていた行のハッシュ
- `meta.projectionContext.writeMode`: canonical / annotation / proposal

必要に応じて以下も持てる。

- `meta.corrects`
- `meta.retracts`
- `meta.proposalId`
- `meta.reviewStatus`

### 13.8 Insert / Update / Delete Semantics

Projection 上の編集操作は、Lake 上では次の意味に正規化される。

| UI Operation | Lake Semantics |
|---|---|
| Insert | 新しい Observation の追加 |
| Update | 旧 Observation を参照した correction Observation の追加 |
| Delete | retraction Observation または状態終了 Observation の追加 |

したがって、どの Projection でも「行を消す」「行を書き換える」という操作は、Ground Truth の破壊的変更を意味しない。

### 13.9 Consistency Laws

Writable Projection と Write Adapter は、少なくとも以下の law を満たさなければならない。

1. **Replay Law**
   同じ Observation 列を再生したとき、Projection の結果は常に一致しなければならない。

2. **No Direct Mutation Law**
   Projection の materialized storage への直接更新を正史として扱ってはならない。

3. **Put-Then-Get Law**
   Write Adapter が受理した操作は、Observation append 後の Projection 再計算結果に反映されなければならない。

4. **Idempotency Law**
   同一 `idempotencyKey` の再送は結果を二重化してはならない。

5. **Provenance Law**
   Projection 上の操作から生成された Observation は、その操作元 Projection と可視コンテキストを追跡できなければならない。

### 13.10 Draft Workspace

利用者がスプレッドシート的にまとめて編集したい場合に備え、Projection とは別に **Draft Workspace** を持ってよい。Draft Workspace は mutable であってよいが、以下の制約を持つ。

- Draft は Ground Truth ではない
- Draft の保存内容は Publish されるまで Projection の正史に反映されない
- Publish 時には差分が Command / Observation 群へ変換される
- Publish 後の正史は Lake に append された Observation のみである

これにより、人間にとっては spreadsheet 的編集体験を維持しつつ、システム全体としては append-only と deterministic replay を保てる。

### 13.11 Composite Projection Insert Policy

他の Projection を合成した DB に対して新規レコードを追加したい場合、次の判定順序を適用する。

1. **Lossless Inversion Possible**
   追加行を canonical Observation 群へ損失なく逆変換できる場合は canonical mode を使う。

2. **Derived Annotation Only**
   追加行が派生結果への注釈・判定・レビューを表す場合は annotation mode を使う。

3. **Ambiguous Semantics**
   元の事実へ一意に戻せない場合は proposal mode を使う。

4. **No Valid Inversion**
   上記のいずれにも該当しない場合、その Projection は当該操作を reject しなければならない。

### 13.12 Inverse Mapping Policy for Mutable Multimodal Sources

Google Slides、スプレッドシート、その他 mutable な multimodal source に対する write-back は、通常の writable projection より厳格に扱う。

基本方針:

- mutable multimodal source は **default read-only**
- write-back を許可するのは、source-native operation に **損失なく** 逆変換できる場合のみ
- 画像や LLM 解釈のみに依存する編集要求は canonical にしてはならない

Google Slides の場合、canonical write-back を許可できるのは次のような操作である。

- 既知 `objectId` を持つ text box のテキスト更新
- 既知 `objectId` を持つ shape/image の属性更新
- slide の並び替え
- speaker notes の更新
- 既知の linked chart の refresh

一方、次のような操作は proposal または annotation に降格する。

- slide 画像だけを見て「このあたりにタイトルを追加してほしい」といった曖昧な編集
- 合成 DB の行から、対応する slide object を一意に特定できない編集
- LLM による意味解釈を前提とした自由レイアウト変更

canonical write-back の必須条件:

1. `presentationId` / `source_document_id` が特定できること
2. 対象 slide の `pageObjectId` が特定できること
3. 対象 element の `objectId`、または同等の stable anchor が特定できること
4. 元の `source_revision_id` または等価な snapshot hash が保持されていること
5. Write Adapter が deterministic な source-native API request 列へ変換できること

運用上、Google Slides への canonical write-back は、画像から slide を再生成することではなく、**native structure に anchored された API request を生成すること** を意味する。

### 13.13 Recommended Default Policy

システム全体の既定値として、以下を推奨する。

- 全 Projection は default read-only
- `writeBack.enabled: true` を明示した Projection のみ writable
- canonical mode は運用チームが管理する Projection に限定
- 個人研究用 / 実験用 Projection は annotation mode または proposal mode を優先
- 正規 Schema が未定義の新規追加要求は、まず proposal mode で受ける

この方針により、関数型プログラミングに近い「不変の入力 + 純粋な導出 + 副作用の隔離」という構造を保ちながら、現実的な追加・編集要求を受け止められる。

---

## Appendix A: Glossary

| Term | Definition |
|---|---|
| **Observation** | Lake に記録される不変の事実単位。「誰/何が、何について、何を観測したか」 |
| **Entity** | 観測の対象。EntityType で型付けされる（例: person:tanaka-2026） |
| **EntityType** | Entity の分類（例: et:person, et:room）。誰でも追加可能 |
| **Schema** | Observation の payload 形式を定義する JSON Schema |
| **Source** | Observation を Lake に送り込む主体（センサー、Bot、人間、API） |
| **Projection** | Lake / 他の Projection から構築される派生 DB |
| **Projection Spec** | Projection の定義ファイル（YAML + 変換コード） |
| **Lake** | Observation の不変ストア（Observation Lake） |
| **Registry** | EntityType / Schema / Source / Projection のメタデータ管理層 |
| **Blob** | Object Storage に格納されるバイナリデータ（画像、音声等） |
| **BlobRef** | Blob への参照。`blob:sha256:<hash>` 形式 |
| **DOI** | Digital Object Identifier。学術引用用の永続識別子 |
| **DAG** | Directed Acyclic Graph。Projection 間のソース依存関係 |

## Appendix B: Design Decisions Log

| Decision | Rationale | Alternatives Considered |
|---|---|---|
| UUID v7（v4 ではなく） | 時刻ソート可能、イベント順序と自然に整合 | UUID v4（ソート不可）、ULID |
| EntityRef 形式 `type:id` | 可読性と型安全性の両立 | URI 形式、数値 ID |
| Schema を JSON Schema で定義 | 広く普及、バリデーションツールが豊富 | Avro（バイナリ最適化寄り）、Protobuf |
| DAG（固定レイヤーではなく） | DB on DBs の柔軟性を最大化 | L1/L2/L3 固定階層（合成の深さが制限される） |
| CAS (Content-Addressable) for Blobs | 自動重複排除、参照の不変性 | パスベース保存（重複・リネーム問題） |
| Opt-Out を 3 段階に分離 | 研究用途では Anonymize/Pseudonymize が Drop より有用 | Drop のみ（ネットワーク分析でグラフ構造が崩壊する） |