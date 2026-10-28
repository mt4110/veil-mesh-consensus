# GCP承認A：受領・課金前準備とCLI認証待ち

> 後続: 本人の再認証後、[承認Aを実施したがVM作成503で停止し、新設networkを撤去した](2026-10-26-GCP-A-VM-CREATE-503.md)。以下は認証待ちだった時点の準備記録として保全する。

2026-10-26 JST。Sol／High、分担なし。[二台構成案](../local-ban/GCP_TWO_VM_EXECUTION_PLAN.md)の承認Aを本人から受領した。最終ゴールは公開経路での安全な試験導入。今回の節目は、最大四時間・増分US$1の範囲で既存resourceを保全して二台を準備・隔離確認し、停止・撤去を記録すること。

**現時点では課金resourceを作成していない。4時間枠のT0は未開始。CLIの認証更新が`invalid_grant`で失敗したため、本人の再認証待ちである。隔離Linuxの実試験、18443の公開、DNS保存、公開証明書発行は未実施。**

## 承認と費用の追加指示

- 承認Aは、指定project／zone、二VM・二新規boot disk・専用VPC／subnet・限定SSH、新規専用鍵、準備と隔離確認、最大四時間／US$1、期限付き削除と所有確認付き撤去の範囲。
- 承認Bは未受領。18443の公開、DNS、公開証明書、公開入口でのBANへ進まない。
- 本人から「100ドルへ行きそうになったら停止し、価値に見合うか検討したい」と追加指示を受けた。**今回のUS$1枠をUS$100へ引き上げたとは扱わない。** 今後の課金試験を含む累計費用がUS$100へ近づく見込みとなる拡張前に、作業を止めて費用対効果を本人と再判断する。
- 過去を含む累計の実請求額や、モデル利用料・他projectとの合算範囲は未確認。今回の手元観測をaccount全体の常時課金監視や、強制的なUS$100上限機能として説明しない。

## 実施した読取りと停止点

Mac上のgcloudは利用可能で、active accountは一件だった。`veil-x-tests`のVM一覧を読取り要求したが、credential refreshが`invalid_grant: Bad Request`で失敗した。VMの作成・削除要求は送っていない。CLIの共有default projectやaccountを別値へ変更していない。

本人へMacのターミナルで`gcloud auth login`を行うよう依頼した。パスワード・認証コード・tokenをチャットへ送らず、通常の本人認証内で完了してもらう。ADC、service account key、追加IAM roleの発行で代替しない。

## 認証を待たずに行った準備

新規private領域`artifacts/gcp-a-prep-9olhrkec/`へ、既存fixtureを上書きせずコピーした。元の実XDP記録と入力は保全している。

| 準備物 | 確認したSHA-256 |
| --- | --- |
| Linux `veil-local-ban` | `e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890` |
| C XDP object | `2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f` |
| 先行hp試験のhost非依存driver、無変更 | `fcf851556d39f399eaab2f09ae6187eac55561b3e69a882a251ebfbb9c51de00` |
| 現行service profile、無変更 | `a59e589a10437b934440ed9500c66adff1931a0b8a3f49ccc2de6aa1978ffd29` |

元のpi専用driverは既存account・namespace・fixtureへ依存するため、そのまま新VMで実行しない。先行hpで使用した別のhost非依存driverは、引数の新規`/run`領域と、一時runnerのPrivateNetworkを使う構造であることを読んだ。

このコピーへ[拒否経路のオフライン検証](../../artifacts/gcp-a-prep-9olhrkec/offline_guards.py)を追加し、次の四項目が成功した。

| 入力・条件 | 結果 |
| --- | --- |
| 非root | コマンド実行前に拒否 |
| hostと同じnetwork namespace | コマンド実行前に拒否 |
| lo以外のinterfaceがあるnamespace | link一覧の読取り以外は実行せず拒否 |
| binary／objectのhash不一致 | link一覧の読取り以外は実行せず、unit・network作成前に拒否 |

os identity／namespaceとcommand adapterは模擬しており、実際のroot、network、systemd、SSH、外部通信を使っていない。[結果](../../artifacts/gcp-a-prep-9olhrkec/offline-guards-result.txt)と[準備manifest](../../artifacts/gcp-a-prep-9olhrkec/preparation.json)を保管。四項目をGCP上のXDP・通信・復旧の成功に数えない。

新しいSSH鍵はまだ生成していない。証明書・TLS秘密鍵・認証tokenの取得や表示なし。既存のPi・hp・router・Macのnetwork設定・製品コードへの変更なし。

## 再開位置

1. CLI再認証後、project・権限・既存resource・同名VM／disk／network／ruleの不存在、image、quota、M/32、適用単価と税／通貨を改めて確認する。
2. 承認された入力と一致し、所有照合と撤去準備が揃ってからT0を記録する。不一致を勝手に修正・権限拡大して作成しない。
3. Aの指定対象だけを作成し、期限付き削除・disk auto-delete・service accountなし・限定SSHを読み戻してから準備と隔離受入れへ進む。
4. Bの条件と実値が揃えば別に確認する。Bが得られない場合や準備上限に達した場合は、Aで承認された停止・所有確認・撤去を行う。今回のA承認を同じ範囲で聞き直さない。

この報告時点では停止・撤去すべき新規cloud resourceはなく、既存のCompute APIと生成済みdefault resourceはそのまま保全している。新規resourceの作成結果・隔離受入れ・撤去は、実施後の別報告へ記録する。

[試験履歴](../local-ban/TEST_HISTORY.md)へ戻る。
