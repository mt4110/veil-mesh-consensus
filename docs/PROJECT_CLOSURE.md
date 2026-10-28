# 開発終了記録 — veil-mesh-consensus

**終了日：2026年10月27日（JST）。開発終了・実験記録として保全。**

この文書は、本人が合意した終了判断、保存済みのクラウド・両ホストの終了確認、意図して残したものをまとめるローカルの最終記録である。今回、新しいSSH接続・クラウド照会・機能試験は行っていない。アーカイブ済みGitHubへの追加送信も行わない。

## 終了判断と未達の目的

当初の目的は「複数サーバーが連携し、1台の侵害によって健全な他サーバーへ不当な遮断を波及させない仕組み」だった。その実用としての成立は未確認で、最終ゴール未達のまま開発を終了する。

後続のローカル手動一時BANには、特定環境での遮断・期限・解除・再起動・復旧の実験成果がある。ただし当初の分散防御の代替にはならない。特権操作とカーネル・NIC・ドライバー・クラウド環境への依存に伴う導入・保守負担に対し、既存手段を上回る効果を示せなかったため、追加開発・適合試験・課金実験を終了した。

「sudoを使うソフトウェアはすべて不適切」「GCP・AWSでeBPFが一律に禁止されている」という結論ではない。汎用クラウド適合と公開サイトへの安全な限定導入は未達。過去の導入手順、未実行の試験案、ロードマップは研究履歴として残し、開発継続のタスクには扱わない。

## 保存済みの終了確認

| 対象 | 確認日時（JST） | 記録された結果 |
| --- | --- | --- |
| GCP `veil-x-tests` | 2026-10-27 12:18 | project集約でVM・disk・予約IP・snapshot等0。試験用network残存なし |
| AWS アカウント末尾5221／us-east-1 | 2026-10-27 12:27 | instance・volume・予約IP・snapshot・自己所有AMI・NAT・ENI各0。試験network・専用Scheduler・IAM role・一時権限の残存なし |
| GitHub | 2026-10-27のアーカイブ作業 | README 2件の終了告知を公開し、isArchived=trueを読み戻して確認 |
| hp-ubuntu | 2026-10-27 13:36 | 稼働試験daemon・試験待受けなし。host NICのXDPとglobal inventoryのXDP型programは0。該当する試験常設unit・named試験namespaceは検出なし |
| pi-node | 2026-10-27 13:36 | 稼働試験daemon・試験待受けなし。host NICのXDPとglobal inventoryのXDP型programは0。下記4 unitは停止・disabled。既存試験namespace内もprocess・待受け・XDPは0 |

これは各確認時点・記載した対象範囲の記録であり、今回の再照会ではない。クラウドの全service・全region・アカウント全体の請求額0を示す記録ではない。既に発生した試験料金は別途請求され得る。

根拠：[クラウド終了確認](review/2026-10-27-CLOUD-STOP-CURRENT-AUDIT.md)、[両ホストの読取り報告](../artifacts/host-stop-readonly-smf108tz/REPORT.md)、[両ホストの時刻・集計](../artifacts/host-stop-readonly-smf108tz/summary.json)、[アーカイブ時の照合記録](../artifacts/archive-preservation-0ukprb3d/verification.json)。

## Piに意図して保全したunit・namespace

### 常設unit 4件

すべて `LoadState=loaded`、`ActiveState=inactive`、`SubState=dead`、`MainPID=0`、`UnitFileState=disabled` を確認した。

| unit | 配置先 |
| --- | --- |
| `veil-local-ban-pi-test.service` | `/etc/systemd/system/veil-local-ban-pi-test.service` |
| `veil-local-ban-pi-full.service` | `/etc/systemd/system/veil-local-ban-pi-full.service` |
| `veil-local-ban-pi-boot.service` | `/etc/systemd/system/veil-local-ban-pi-boot.service` |
| `veil-local-ban-pi-namespace.service` | `/etc/systemd/system/veil-local-ban-pi-namespace.service` |

`test`と`full`の`.service.d/10-isolation.conf`・`90-recovery.conf`も保全。`list-unit-files`のpreset列のenabledは現在のdisabledと別の値であり、自動起動が有効という意味ではない。

配置済みの`/opt/veil-local-ban-pi-{test,full,boot}`と`/var/lib/veil-local-ban-pi-{test,full,boot}`も残した。存在とメタデータを確認したもので、保存データの本文や秘密鍵を開いた調査ではない。

### 既存namespaceとveth

- `/run/netns/ns-veil-rogue`を維持。対応mountはactiveだが、BANサービスの稼働を意味しない。
- namespace内の`veth-rogue`とhost側`veth-victim`は残り、UPを確認。namespace内のprocess・待受け・XDPは0。
- 以前の常設試験計画でも「既存namespace／veth」として扱っていたfixtureである。本プロジェクト専用に新規作成したものとは断定せず、他用途の保全のため停止・削除しなかった。

**停止すべき試験サービス・XDPは見つからず、追加の停止・解除は不要だった。** 将来のための対象限定stop手順は[読取り報告](../artifacts/host-stop-readonly-smf108tz/REPORT.md)に保全してあるが、今回は実行していない。namespace・vethの撤去も今回の作業や残タスクに含めない。

## 既存サービスとデータの保全

確認時点でhpのSSH・systemd-networkdはactive/running。PiのSSH・Dockerはactive/running、pi-mainnet・WireGuardのunitはactive/exitedで記録された。これらは各アプリの業務機能・ブロックチェーン同期・VPNの外部到達性まで検証したという意味ではない。

両ホストに残るcgroup／sysctl等の非XDPのBPFや、Piの既存nftables tableは変更していない。WAL・証明書・秘密鍵・既存データ本文には触れていない。最後の両ホスト確認も読み取りだけであり、stop／disable／detach／flush／削除／設定変更は0件だった。

## ローカル保全先と公開版の違い

- 公開リポジトリ：[mt4110/veil-mesh-consensus](https://github.com/mt4110/veil-mesh-consensus)。終了告知コミットは`97bd62dffb508405b03d2d3e7ce10f31eff4da95`。変更はREADME.md・README_EN.mdだけ。
- [開発終了時の保全記録](../artifacts/archive-preservation-0ukprb3d/PRESERVATION.md)に、編集前の作業ファイルtar、Git履歴bundle、binary対応差分、照合manifestと限界を記録した。
- 既存コード・未コミット変更・ignoredの試験証拠はローカルに保持。公開GitHubとローカルの後続修正・証拠は同一ではない。
- 今回の文書追記もローカルのみ。アーカイブの解除・commit・push・公開・新規課金は行わない。

## 終了の区切り

**開発終了の告知、公開リポジトリのアーカイブ、クラウド試験資源の撤去確認、両ホストの稼働試験サービス・XDPの残存確認、保全対象の記録まで完了した。**

未達の機能・未検証の環境は研究の限界として残す。実装・追加試験・fixture撤去を今後の必須作業として積み残さず、ここで本プロジェクトの開発を終了する。
