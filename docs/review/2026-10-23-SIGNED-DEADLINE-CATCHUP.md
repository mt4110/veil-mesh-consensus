# 署名付き共通期限と遅延catch-upの修正・検証

対象: `7e9473c` を基点とした既存の未コミット修正に追加。2026-10-23。旧TTL・WAL修正の検証は[前回記録](2026-10-23-TTL-BYPASS-RECOVERY.md)、他の残課題は[重大欠陥の修正記録](2026-10-23-CRITICAL-FIXES.md)を参照。

最終ゴールは小規模サイトでの実践投入。現在のマイルストーンは期限と復旧の安全性。このタスクの完了条件は、初めて古いログを受け取るノードでも期限切れBANを開始せず、期限を改変できず、回帰試験と隔離Linuxで確かめること。本タスクは完了したが、本番自動遮断の投入判定は保留する。

## 実装契約

相対TTLは提案入力として受け付け、leaderがログへ追加する前に `SetPeerTrustLeaseBatch` へ変換する。次の値をログの同じ内容として全ノードへ配る。

| 値 | 意味 |
| --- | --- |
| `issued_at_unix_ns` | バッチ共通の発行時刻。POSIX/UTCのepochからの整数ns |
| `rule.ttl_secs` | 各ルールの要求期間。既存の上限86,400秒を維持 |
| `not_after_unix_ns` | 各ルールの共通期限。TTLが正なら発行時刻 + TTL、TTLが0なら0 |

これらは既存のログchain hash、Ed25519 RPC署名、QCが署名するhashの対象となる。受信ノードが発行時刻を書き直すことはない。すでに期限を含む提案を再送しても再発行しない。期限延長には新たな決定が必要であり、相対TTLを再び提案する操作は新規決定として扱う。入力API全体のdecision IDによる重複排除は今回の対象外。

構造検証は全ノードで決定的に行う。発行時刻0、不正な期限計算、整数overflow、既存の保護対象・バッチ件数・TTL上限違反を投票・ログ変更前に拒否する。各ノードの現在時刻はこの一致判定へ混ぜない。正しい署名とQCを持つ期限切れログはcatch-upしてcommitを進めるが、ローカルの遮断は開始しない。

## ローカル執行と時計の前提

各ノードはboot clockを先に読み、続いて壁時計とその誤差上限を使う。単位はすべてns。上限時刻を `upper = unix_ns + uncertainty_ns` とする。

- `issued_at > upper` または `not_after <= upper` は適用せず、期限値1を失効記録として保存する。未来の決定を予約して後から発動することもない。
- 有効なら `remaining = not_after - upper` を計算する。boot clockが実時間より遅い率の上限を `rho` とし、`floor(remaining × (1 - rho))` を先に読んだboot値へ加える。端数切捨て・先行boot読取りはいずれも期限を早める方向になる。
- Linux実装では `rho = 1000 ppm = 0.1%` を**運用上の前提**として使う。これは今回測定した誤差率でも、任意の機械・VMで保証された数値でもない。時計disciplineと発振器の誤差を含め、この上限を満たす必要がある。
- 一度保存した同じ `(log index, rule ordinal)` の期限は、再計算値との小さい方しか採用しない。WAL checkpointをカーネル適用より前に保存する。再起動、適用再試行、重複heartbeatで延長しない。
- OS bootが変わった場合や必要な期限記録が欠けた復旧では、前回と同じく抑止する。完全なWAL消失やバックアップ巻戻し全体の検知を保証するものではない。

`LinuxBootClock` は `adjtimex(modes=0)` で状態を照会するだけで、時刻やNTP設定を変更しない。TIME_OKで、未同期・時計異常・うるう秒遷移のflagがなく、`maxerror` が設定した上限以下であることを要求する。`maxerror` の単位はmicrosecondsなのでnsへ変換する。[Linux adjtimex一次資料](https://man7.org/linux/man-pages/man2/adjtimex.2.html)

通常daemonの上限設定は `--max-clock-error-ms 1000`。正の値を要求する。値を大きくすると、より不確実な時計で動ける代わりに、上限誤差を差し引くため使える残時間が短くなる。設定値が十分大きくても、未同期状態は許可しない。time namespaceのboottime offsetが0であることも確認し、カーネルBPFとの基準不一致を避ける。

**kernelが報告する誤差を取得できたことは、正しいUTCを暗号的に証明したことではない。** NTPの時刻源・ホスト管理者・上記速度上限を信頼する。任意のclock step、侵害された時刻源、ホストrootによる改変まで期限を保証するものではない。適用済みルールは固定boot期限で進み、壁時計の変更に追従して延長しない。適用後の任意の時刻異常を監視して全ルールを再計算する機能は追加していない。

時計品質を確認できないとき、leaderの新規TTL提案は失敗する。受信側は合意履歴を進めつつ新規遮断を抑止し、その結果を永続化する。同じログ位置は時計が回復しても再武装しない。必要なら新しい承認・決定を行う。`lease_clock_error` はdegraded判定へ反映するが、metrics HTTP公開は引き続き未実装。各ノードの同時解除は保証せず、安全側に早く解除するノードがあり得る。

## WAL・wire互換性

後続修正: 以下は共通期限追加時点の契約。[クラスタ・構成世代binding](2026-10-23-CLUSTER-CONTEXT-BINDING.md)の追加後はWAL v3となり、旧v1/v2は自動移行せず読込みを拒否する。

`MeshCommand` の新しいvariantを末尾に追加した。既存variantのbincode番号、WAL v2の外側形式を保持し、過去のログbytes・hash・投票記録を勝手に書き換えない。

旧形式の相対TTLだけを持つログには、認証された共通期限がない。そのため、**旧WAL v2は読めるが、そのTTL付きBANは保存済みローカル期限が残っていても再武装しない**。稼働中の遮断が減る移行になるため、基礎firewall/WAFを維持し、必要なBANは新形式の決定として再承認する。TTL=0の旧形式・無期限ルールは既存互換性のため残る。すべてのBANが期限付きになったという変更ではない。

旧WAL v1は従来どおり拒否する。WALの削除や投票記録のリセットで回避しない。旧バイナリは新しいvariantをdecodeできないため、混在クラスタや新形式記録後の旧バイナリへのrollbackは非対応。保全したWALを使い、全ノードを同じ対応版へ更新する運用が必要。自動移行・rolling upgradeの実証は行っていない。

## 回帰検証

| 検証 | 結果・範囲 |
| --- | --- |
| macOS `cargo test --workspace --locked --offline` | 57 passed / 0 failed / 1 ignored |
| pi-node Linux同コマンド（対話・既存専用Linux試験3件を除外） | 57 passed / 0 failed / 4 ignored / 3 filtered |
| macOS / Linux `cargo clippy --workspace --all-targets ... -- -D warnings` | 両方成功 |
| `cargo fmt --all -- --check` / `git diff --check` / Markdown lint | 最終確認を実施し証拠に記録 |
| `linux_delayed_catch_up_signed_deadline` | 明示root実行で成功。実mTLS QUIC + 実XDP |
| `linux_live_ttl_bypass_recovery` | 明示root実行で成功。停止中TTL失効・WAL・バイパス・通常daemon/CLIの既存回帰 |

通常回帰に追加した11件は、期限後catch-up、boot基準が異なる受信側の残時間、RPC/QCの改変拒否、不正な提案・署名済み不正な受信window、誤差・速度・境界・overflow、時計不明時の発行拒否と永続的な適用抑止、期限済み提案の再送、WAL復元時の時計前進/後退、旧WALのhash・投票保全、64KiB上限の分割catch-upを覆う。Linux専用の時計品質試験も追加した。

途中の試験失敗では、protocolの拒否応答がRustの `Err` ではなく `success=false` であることと、発行時刻より前へ時計を戻した場合の抑止条件を期待値へ反映した。拒否時のログ・投票非変更、空のQC署名という安全条件は維持している。試験数をBFT全体の正しさの証明とは扱わない。

## pi-nodeの実パケット確認

Linux 7.0.0-38-generic / x86_64、Rust 1.98.1。既存の隔離 `veth-victim` / `ns-veil-rogue` を使用。4ノードは同一ホスト内の独立state・WAL・鍵とloopback mTLS QUICで動く。遅延ノードだけがvethへXDPを所有FDでattachする。ホスト時刻・NTP設定・物理NIC・既存firewallは変更していない。

1. leaderと2ノードでTTL 3秒を確定し、遅延ノードにはまだログを配らない。
2. 署名された期限を過ぎてから元のログ・QCをQUICで配る。遅延ノードは同じhashでcommit/applyを完了するが、pingはPASS、DROPカウンタは0。heartbeatでも再武装しない。
3. 対照として新しいTTL 5秒の決定を配り、実DROPを確認する。署名期限後にpingがPASSへ戻る。観測カウンタはPASS=3、DROP=1。
4. 通信endpoint・所有XDPを解放し、XDP attachmentなしとping成功を確認する。既存veth/namespaceは保持する。

観測時のkernel報告誤差は173,000,000ns。これはその照会時点の値である。単一Linuxホスト上のこの試験は、多ホストのclock skew、別kernel/CPU、長期運用、真のByzantine耐性や本番保護の証明ではない。時計異常とboot差分はfake clockによる決定的試験であり、実ホストの時刻を変更した実験ではない。

新形式で既存ライブ試験も通過し、SIGSTOP中のカーネルTTL、kill後の期限切れWAL復旧、ソケットによる緊急バイパス、バイパス復元と明示off、通常daemonのWAL/CLI経路を再確認した。実XDP試験はこの修正後のruntimeコードで実施し、その後の変更は追加回帰テストとコメント・文書のみ。

## 証拠と残る判断

ローカル証拠: `artifacts/signed-deadline-29lk2nvn/`（git管理外）。開始時の `before.tar` / `before.patch` / `before-hashes.json`、実機出力 `linux-first.txt`、最終回帰・Clippy `linux-final.txt`、最終source manifestと差分を保全する。実機検証コピーは `/tmp/veil-deadline-zcV7LV7m`。元の `/home/maakie/veil-mesh-consensus` は編集していない。生成されたPKI秘密鍵は証拠archiveへ収集しない。

残る重要事項はBFTのview-change/進行性、cluster/config epochへの署名binding、通常daemonの入力・同期・観測の運用経路、site固有の保護対象、容量・負荷・運用受入れ。小規模サイトの構成は未決定であり、基礎防御を維持した隔離試験の範囲を超える投入判断は今回行っていない。

## 後続の有限BAN必須化

[有限TTLの合意経路全体での強制](2026-10-24-FINITE-BAN-POLICY.md)により、旧形式・TTL=0のBANを新規受信/適用しない。禁止BANを含むWALは保全して起動を拒否する。以前の無期限BAN互換や旧相対TTLだけのWAL復元の記述より、この後続契約を優先する。
