# 開発終了 — veil-mesh-consensus

> [!CAUTION]
> **2026年10月27日、開発を終了しました。実験・研究用の記録です。本番環境への導入を推奨しません。**
>
> **今後の保守・機能追加・セキュリティ修正は予定していません。GitHubは読み取り専用のアーカイブとして保管します。**

## [ARCHIVED] The Slumbering Destroyer (2026)

本リポジトリは、**eBPF × Rust × QUICによる合意形成**を用い、自律型メッシュ防衛システムを目指した実験的プロトタイプです。

[Cloud Runのようにカーネル操作を許可しないマネージド環境](https://docs.cloud.google.com/run/docs/container-contract#container_security_restrictions)では、この方式をそのまま稼働させることはできません。VMやベアメタルでも、本実装の稼働はカーネル・NIC・ドライバーの条件に依存し、本番運用の成立は未確認です。

しかし、いつかDePIN（分散型物理インフラネットワーク）やP2Pベアメタルクラウドが当たり前になるWeb3の時代が訪れ、インフラの奥底まで使い手が自ら選べるようになったなら——。

**その時、この実験を受け継ぐ者よ。この「駆逐艦」の封印を解いてください。**

**それまで、このリポジトリは深い眠りにつきます。**

## 開発終了の理由

- 当初の目的である「複数サーバーが連携し、1台が侵害されても健全な他サーバーへ不当な遮断を波及させない仕組み」は、実用としての成立を確認できませんでした。後続の単体ホスト向け手動一時BANは、この目的の達成を意味しません。
- 特権操作とカーネル・NIC・ドライバー・クラウド実行環境への依存が強く、広く安全に導入・保守する負担に見合う効果を、既存手段との比較で示せませんでした。
- 隔離環境や特定ホストでの成功と失敗は研究成果として残しますが、汎用クラウド適合・公開サイトへの安全な試験導入は未達です。追加開発・追加課金試験は終了します。

これは「sudoを使うソフトウェアはすべて不適切」「GCP・AWSでeBPFが一律に禁止されている」という結論ではありません。本プロジェクトの目的・実装・導入負担・確認済みの効果を合わせた終了判断です。

コードと履歴、既存の未コミット変更、ローカルの試験証拠は保全します。GitHubのコードとローカルの後続修正・試験記録は同一ではありません。**以下の手順・設計・ロードマップは過去の実験記録です。確認済みの範囲と未達事項は[公開コード・試験記録の確認状況](docs/VALIDATION_STATUS.md)を参照してください。現在の導入推奨や開発再開計画ではありません。**

---

## 🛡️ veil-mesh-consensus

[![CI Checks](https://github.com/mt4110/veil-mesh-consensus/actions/workflows/ci.yml/badge.svg)](https://github.com/mt4110/veil-mesh-consensus/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/mt4110/veil-mesh-consensus)](https://github.com/mt4110/veil-mesh-consensus/releases)

バッジのCI状態・Release版番号は履歴の入口です。実kernel試験の実行や本番受入れを保証しません。

[English](README_EN.md) · [🧭 旧設計・終了したロードマップ](docs/V1_ARCHITECTURE_AND_ROADMAP.md) · [📋 旧版レビュー・未完の改訂案](docs/FEASIBILITY_REVIEW_AND_V1_DESIGN.md) · [☁️ クラウド・エッジの過去の構成案（導入・連携未検証）](docs/CLOUD_DEPLOYMENT_GUIDE.md) · [🧪 シナリオ試験の範囲と制約](docs/CONSENSUS_AND_EBPF_PROOF.md) · [🔒 Security](SECURITY.md) · [🤝 Contributing](CONTRIBUTING.md)

署名付き合意の状態機械、mTLS QUIC、IPv4 XDP、TUIを試作した実験的プロトタイプです。これらの部品・限定シナリオの試験があることと、侵害耐性のある分散防御が運用可能であることは別です。

![過去のTUI実験の説明図。分散防御・機密漏洩防止の保証ではありません](./assets/extermination-flow.svg)

---

## 🎬 過去の隔離veth・TUIデモ

<!-- rgc:start -->
![過去の隔離vethと同一プロセスTUIのデモ](./assets/demo.gif)
<!-- rgc:end -->

これは専用Linux環境の隔離vethとTUIの実演です。独立した4ホストの分散防御や、公開サイトの受入れを示しません。TUIの4状態機械は同一プロセスで動き、デモ用の鍵を使います。

1. `[a]` は試験所有のprocessを起動します。専用namespaceがあるLinuxではUDPを送り、それ以外では `sleep` を使います。約1msの待機は実装上の目安で、1000 pkt/sの実測保証ではありません。
2. `[b]` は不正署名RPCを状態機械へ渡すデモです。実運用の攻撃検知や独立ホスト間の耐侵害性の試験ではありません。
3. `[x]` は状態機械の合意操作、接続されているbridgeへの反映、ダミーbuffer消去、試験が起動したprocessの停止を行います。LSMによるホスト全体の保護や実アプリの秘密鍵消去は実証していません。kernel attach／map操作の成否は表示だけで断定できません。

---

## 🏗️ Veil各製品との組合せは過去の構想

観測→合意→執行→監査という役割分担を構想しましたが、一連の連携実装と受入れ記録は確認できません。各製品単体の機能や成果は、本作の対応保証にはなりません。

| 参照先 | 本作との組合せの確認状況 |
| --- | --- |
| [veil-warden](https://github.com/mt4110/veil-warden) | 観測・ホスト制御との連携は未検証。PHP保護や自動攻撃検知の根拠にしない |
| [veil-rs](https://github.com/mt4110/veil-rs) | 監査ログのスキャン・マスキングとの連携は未検証 |
| [veil-arena](https://github.com/mt4110/veil-arena) | `arena-smoke.sh` は本作のログを保存するscript。Arena全体の受入れ・2VM防御の成立を示さない |

WAFやBlackholeから正当な攻撃元IPを自動取得して遮断する完成した入力経路も確認できません。DNS観測・接続先・HTTPヘッダー・攻撃元のL3送信元を同一視しません。

---

## 🛡️ 実装・試験の範囲と未達事項

[確認状況と公開コードの根拠](docs/VALIDATION_STATUS.md)および[シナリオ試験の解説](docs/CONSENSUS_AND_EBPF_PROOF.md)を参照してください。`proof_*` は限定シナリオのテスト名であり、BFT安全性の数学的証明ではありません。

| 項目 | 公開版で確認できる範囲 | 限界 |
| --- | --- | --- |
| 署名・QC・ログhash | 検査処理と限定した攻撃の回帰テストがある | 誤検知防止・任意の侵害への耐性・分散防御の完成を保証しない |
| mTLS・共有PKI・WAL | 読書きとテスト経路がある | 安全な鍵配布、全応答前の永続化、独立ホスト運用の受入れは未達 |
| View-Change | timeoutを明示駆動する状態機械の試験がある | 通常daemonのtimerによる自動検閲回復、完成した2段階QC BFTは未確認 |
| TTL・緊急bypass・metrics | ユーザー空間の関数と試験がある | kernel自律失効・kernel bypass操作・復旧CLI・HTTP exporterは公開版の完成機能ではない |
| XDP | IPv4送信元HASH。旧版の隔離vethで実測記録がある | IPv6／CIDR、kernel TTL、全NIC適合、実objectと両ソースの一致は未保証 |
| LSM・秘密消去 | Rust hook、任意attach経路、ダミーbuffer消去がある | 実アプリ秘密鍵の消去・PHP／ホスト保護・漏洩ゼロは未実証 |
| ガード | exact IPv4と一部の境界検査がある | 管理網・peer・gateway全体の自動保護ではない |

rootを奪われたホスト単体の保護、定足数の鍵の同時侵害、上流回線の飽和は対象外です。IPv6はXDPの対象外であり、送信元偽装・NAT・共有proxyによる誤遮断リスクがあります。署名や合意は検知の正しさを作りません。

---

## 🖥️ 過去のTUI・専用Linux環境の操作例

以下は研究用の操作例です。`deploy-pi-node.sh` はリモートhostのnamespace・interface・trace設定等を変更し得ます。一般的な導入・安全な復旧手順として検証したものではありません。今回これらを実行していません。

### 1. Mac ローカルでの TUI 起動

```bash
# 対話モードで TUI アリーナを起動 (4ノードクラスタ・シミュレーション)
cargo run -p veil-node -- --tui

# 自動デモ再生モード
cargo run -p veil-node -- --tui-demo
```

### 2. 実機 Linux (`pi-node`: Kernel 7.0) での実カーネル XDP + `iftop` + トレース連動

```bash
# ターミナル1: pi-node の仮想攻撃NIC (veth-victim) を iftop で監視
ssh -t pi-node "sudo iftop -i veth-victim -n -N -P"

# ターミナル2: Linux カーネル内部の SIGKILL (sig=9) & XDP_DROP トレースを監視
bash scripts/deploy-pi-node.sh trace

# ターミナル3: 実カーネル eBPF (veil_xdp_kern.o) をアタッチして TUI アリーナを起動
bash scripts/deploy-pi-node.sh tui
```

### TUI 操作キー一覧

| キー | アクション | 内部で行われる処理 |
| :---: | :--- | :--- |
| **`[a]`** | **試験processの起動** | 専用Linux namespaceではUDP送出、それ以外ではsleep。送出rateは未測定です。 |
| **`[b]`** | **ビザンチン偽造 RPC 注入** | リーダー `node-alpha` を詐称した不正な Ed25519 署名パケットを全ノードへ注入し、`handle_signed_rpc` で棄却されることを確認します。 |
| **`[x]`** | **同一プロセス内の合意デモ** | bridge反映・ダミーbuffer消去と試験所有processの停止。実kernel反映の成功や実アプリ保護は別途確認が必要です。 |
| **`[r]`** | **クラスターリセット** | TUIの状態をリセットします。実kernel mapやホスト状態の完全初期化・通信復旧を保証しません。 |
| **`[q]`** | **終了** | 試験所有のmock processの終了・回収を試みます。ホスト全体の復旧を保証しません。 |

---

## 🧪 シナリオ試験・品質チェックの範囲

[`proof_1`〜`proof_14` と回帰テスト](docs/CONSENSUS_AND_EBPF_PROOF.md)は、状態機械、同一プロセス内のQUIC、Mock bridge、条件付きLinux veth等を扱います。試験名や件数は一般的な安全性の証明ではありません。

```bash
# 開発当時の品質チェックscript（検査内容・skipは個別確認が必要）
bash scripts/check.sh

# 過去の専用Linux環境用。リモート状態を変更し得る
bash scripts/deploy-pi-node.sh test

# cargo test出力と固定形式の要約を保存するscript
bash scripts/arena-smoke.sh
```

`proof_7` は前提不足やattach失敗を `Ok(())` として返します。CIやsmoke要約のPASSから実kernel試験が動いたとは判断できません。`check.sh` はShellCheckが見つからない場合にその検査を実行しません。今回の文書訂正ではこれらの製品試験・実機試験を再実行していません。

---

## 🚀 Nix Flakes + MicroVM の過去の構成例

設定・起動例を保管しています。VMが起動することと、公開mainの独立2VMで分散防御が成立することは別です。後者の運用受入れは今回確認していません。構成には開発用root自動ログインがあり、本番向け設定ではありません。

```bash
direnv allow
cargo run -p xtask -- doctor

# ターミナル1: node-alpha (10.0.0.1/24) の起動
nix run .#node-alpha

# ターミナル2: node-beta (10.0.0.2/24) の起動
nix run .#node-beta
```

---

## ☁️ クラウド・エッジ環境への導入構成（AWS / GCP / Azure / Cloudflare / Vercel）

以下のAWS・GCP実測は、公開mainと異なる後続ローカル版 `veil-local-ban` の結果です。公開mainの分散メッシュの合格ではありません。クラウド側のXDP機能の有無だけでは、本実装の導入成功を保証しません。[AWS・GCP試験の公開用要約](docs/review/2026-10-27-CLOUD-VALIDATION-SUMMARY.md)を参照してください。**[旧導入ガイド](docs/CLOUD_DEPLOYMENT_GUIDE.md)は過去の構成案であり、導入成功を確認した手順ではありません。**

| プラットフォーム | 本実装の直接導入・連携の確認状況 | 試験結果・過去の構成案 | 旧ガイド |
| :--- | :---: | :--- | :--- |
| **AWS** (EC2 / EKS) | **手元のEC2試験では実NICに装着できず** | 隔離XDPは合格。ENA／MTU 9001で装着拒否。EKS・別構成は未検証 | [AWS 過去の構成案](docs/CLOUD_DEPLOYMENT_GUIDE.md#3-aws-amazon-ec2--vpc-への実践導入手順) |
| **GCP** (Compute Engine / GKE) | **手元のCompute Engine試験では実NICに装着できず** | 隔離XDPは合格。VirtIO-Net／MTU 1460でEINVAL。gVNIC・GKE・別構成は未検証 | [GCP 過去の構成案](docs/CLOUD_DEPLOYMENT_GUIDE.md#4-gcp-google-compute-engine--vpc-への実践導入手順) |
| **Microsoft Azure** (Azure VM / AKS) | **未検証** | MANA／Accelerated Networkingを含め本実装の導入成功は未確認 | [Azure 過去の構成案](docs/CLOUD_DEPLOYMENT_GUIDE.md#5-microsoft-azure-azure-virtual-machines--vnet-への実践導入手順) |
| **Cloudflare** (WAF / Workers) | **連携未検証** | 過去の構成案。API遮断同期・オリジン保護の連携実装と受入れ記録は未確認 | [Cloudflare 過去の構成案](docs/CLOUD_DEPLOYMENT_GUIDE.md#6-cloudflare--vercel-を使う場合のハイブリッド実践構成) |
| **Vercel** (Next.js / Edge) | **連携未検証** | 過去の別Linuxバックエンド構成案。連携実装と受入れ記録は未確認 | [Vercel 過去の構成案](docs/CLOUD_DEPLOYMENT_GUIDE.md#6-cloudflare--vercel-を使う場合のハイブリッド実践構成) |
