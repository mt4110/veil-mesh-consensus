# ローカル一時BAN: 専用ユーザー・常設systemd unitの隔離受入れ

2026-10-25 JST。[事前計画と承認対象](2026-10-25-LOCAL-BAN-PERMANENT-SYSTEMD-PLAN.md)に対し、ユーザーの明示承認後にpi-nodeで実施した。
最終ゴールは小規模公開サイトで安全に試用できる無償OSS。現在の節目は、ローカル一時BANの運用・復旧を実環境で確認すること。
今回の起動・停止・再起動・手動復旧の機能確認は完了し、**試験用サービスは停止・disabledで残した。** 実サイトへの本番適用や自動起動を承認したものではない。

## 確認した振る舞い

| 対象 | 実測・確認結果 |
| --- | --- |
| 専用user/group | `veil-local-ban-pi-test`、UID/GID 980。home=/nonexistent、nologin、password locked、追加の補助groupなし |
| 常設unit | `/etc/systemd/system/veil-local-ban-pi-test.service`からロード、Transient=no。停止後の同じunitへのstartも成功 |
| 稼働権限 | 初回・SIGKILL後・復旧先の全threadでCapEff/Prm/Inh/Amb=0、NoNewPrivs=1。CapBndには起動用3 capabilityの上限が残る |
| 通常通信と範囲 | 隔離`10.99.0.66:18443/TCP`へのTLS/IP SAN・run固有bodyを照合。BANで対象入口を遮断し、別port 18444は通信継続 |
| 期限・個別解除 | TTL 3秒でkernel側の失効と通信復帰、分割掃除、明示的なBAN/解除が成功 |
| 容量 | 既存の変更間隔で256件を投入。257件目はCAPACITYとなり、先に登録した実通信元のBANを追い出さないことを確認 |
| 正常restart | 256件のBANが消え、新instance・occupied=0、HTTPS通信復帰 |
| SIGKILL | 対象unitのmainだけをkill。自動再起動し、新instance・BAN 0・HTTPS通信復帰 |
| 所有記録破損 | 今回作成したprimaryだけを破損。診断で修復せず、製品processはexit 1、再起動loopなし。未知fileのfixtureも保全 |
| 手動復旧 | 同じ常設unitに実際の`90-recovery.conf`を配置し再読込み。固定recoveryでBAN 0、HTTPS通信成功 |
| 元領域の保全 | primaryのUID・mode・inode・内容hashが最初の破損fixture snapshotと一致。サービスのmount namespace/UIDからの書込み用openはEROFS |
| 復旧後のrestart | recoveryで実際にBANしてからrestart。新instance・BAN 0・通信復帰。元primaryは変化なし |
| 最終状態 | ActiveState=inactive、MainPID=0、UnitFileState=disabled、XDP非装着、専用UIDのprocessなし、試験portの待受けなし |

製品binaryは前回と同じSHA-256 `f9cf2dc594cc0808b91247a8f0d447edcb7b9370ae8273e0f777deebe85da05a`。
objectも同じ`2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f`。Rust/BFT/WAL・依存・TTL・処理期限は変更していない。

## 試験driverの失敗と、証拠を保全した続行

**単一の通し実行が最初から最後まで成功したとは報告しない。** 必要な振る舞いを、以下の3記録で確認した。

1. 前半26項目が成功した後、`systemctl reset-failed`が「Unit not loaded」で終了したことをdriverが例外として扱い停止した。全体complete=falseで保存。停止後も常設fileは残るが、inactiveなunitのメモリ上の状態はGCされる。ResetFailedUnitはfileを自動loadしない。
2. 無条件にerrorを握り潰さず、当該応答に限り、正しいFragmentPath・inactive・MainPID 0・Result success・NRestarts 0を確認して扱うよう修正した。元primaryのidentity/hash、空recovery、未配置の復旧drop-inを照合して復旧部分だけ続行。7項目成功後、続行経路の`proof`変数の初期化漏れで再停止した。これもcomplete=falseで保存。
3. 変数初期化を修正し、既存drop-inのhash、recovery所有記録のtype/UID/GID/mode/size/link数と製品のread-only診断、元primaryのsnapshot一致を確認して残りだけ続行した。最後の12項目は成功、complete=true、cleanup_ok=true。

`--resume-result`は上記の既知のdriver停止点に限る。fixtureが変わっていたり、未知file・異なる定義があったりすれば拒否する。再インストール、証拠の削除、primaryの修復、命令の自動再送をせず、失敗記録も残した。
最後のcomplete=trueは**その続行区間の完了**であり、前の2記録を成功に書き換えるものではない。
集約記録は`required_behaviors_confirmed=true`と`full_single_run_passed=false`を分けて持つ。

この前回試験時点では、修正版driverの単一通し実行は未実施だった。今回のデータを消して通し直すことはしていない。
その後、別途承認した新規対象`veil-local-ban-pi-full`で[途中再開なしの通し試験に成功した](2026-10-25-LOCAL-BAN-FULL-RUN-ACCEPTANCE.md)。この前回の3記録は、分割実行としてそのまま保全する。

## 回帰と検査範囲

- 既存Linux library回帰: **24 passed / 1 ignored / 0 failed**。ignoredは従来の明示SIGKILL用fixture。今回は製品コードを変更していない。
- 新しいdriverのerror分類: **5項目成功**。正しいinactive/未ロード応答だけを許容し、権限error・active状態・failed状態・別FragmentPathでは停止する。
- 配置用I/O helperは準備時に4項目を確認済み。O_EXCL、新規fileのみの明示mode、symlink/既存file/既存directory保全を確認した。
- 常設unitの実配置と、復旧drop-in配置後の`systemd-analyze verify`は成功。nobodyを代用していない。
- 既存BFT binaryの全体回帰は今回再実行していない。前回のQUIC期限切れとその後の成功から、BFTのタイミング問題が解消したとは扱わない。

## 既存データ・他サービスの保全

承認した新規user/group・専用配置先だけを作成した。アカウントDBはsystemd-sysusersで更新し、shadow/gshadow本文は表示・収集していない。
配置前のpasswd/groupの既存entryは、内容を出力せず行hashで比較し、既存entryが残っていることを確認した。DB全体をbackupで巻き戻していない。

SSH・Docker・systemd-timesyncd・UFWのLoadState/ActiveState/MainPIDは、**配置直前と最終監査で一致**した。これらへのrestart/stop/config変更は行っていない。
host namespaceのinterface名・ifindex・flags・XDP情報も前後で一致した。装着先は既存`ns-veil-rogue`内のvethだけで、最後は非装着。
これは上記検査範囲の保全確認であり、全host file・全process・secret DB内容の完全不変を証明するものではない。

## ホストに残したものと、その後の扱い

- 専用user/group `veil-local-ban-pi-test`（UID/GID 980）。削除・UID再利用はしない。
- `/opt/veil-local-ban-pi-test/`: root所有のbinary/object/config、sysusers定義、復旧drop-in原本、installation manifest。
- `/etc/systemd/system/veil-local-ban-pi-test.service`と`.service.d/{10-isolation.conf,90-recovery.conf}`。
- `/var/lib/veil-local-ban-pi-test/primary`: **意図的に壊したfixtureと未知fileをそのまま保全**。
- 同`recovery`: 現在選択されている管理先。停止後の72-byte所有記録を保持する。BANは保存していない。
- privateな試験ログと証拠。試験用TLS鍵もprivateなremote試験領域に保持し、報告へ本文を取り込まない。

unitをenableしていない。今後明示的にstartする場合、現在は90-recovery.confが有効で、管理clientもrecoveryを指定する。primaryを手作業で修復して戻さない。

```sh
# 導入先管理者が改めて実行する場合の例。今回の最終状態は停止のまま。
sudo systemctl start veil-local-ban-pi-test.service
sudo -u veil-local-ban-pi-test /opt/veil-local-ban-pi-test/veil-local-ban status \
  --control-dir /var/lib/veil-local-ban-pi-test/recovery
sudo systemctl stop veil-local-ban-pi-test.service
```

start前には隔離namespace/interfaceの存在と他XDPの不在を確認する。管理userの設定や対象service scopeが変わっていれば停止する。
完全撤去、enable、ホスト再起動、物理NICや本番入口への設定変更は今回の承認に含まない。

## systemdの回数制限についての補足

`Restart=on-abnormal`、StartLimitIntervalSec=60、StartLimitBurst=3は変更していない。
ただし**unitがGCでunloadされるとrate-limit counterも消える**。永続DBによる「host全体で必ず毎分3回まで」という意味ではない。
pi-node備付けのsystemd.unit man（259.9）の該当説明も一次証拠へ保存した。手動stop/startやreset-failedを跨ぐ永続的な起動予算は実装していない。

## 判定と残る課題

**専用UID・常設unitでの今回の隔離機能確認は完了。** 256件上限、揮発BAN、元データ保全、実際の手動切替、停止/disabledの最終状態を確認した。
修正版driverの単一通し再実行は、その後の[新規対象による受入れ](2026-10-25-LOCAL-BAN-FULL-RUN-ACCEPTANCE.md)で完了した。残るのは実サイトの入口/保護IP/基礎防御を決めた受入れと、別途承認が必要なホスト再起動/自動起動の確認である。
既存の`/run/netns/ns-veil-rogue`を前提にした試験unitを、そのまま本番用・再起動後も自動復元できる構成と扱わない。
IPv6、CDN越しのclient IP、攻撃検知の精度、帯域を埋める攻撃への耐性は、このIPv4/TCPのローカルBAN試験から保証しない。

## 一次証拠

- local: `artifacts/permanent-systemd-c93fa_mq/`
- 集約: `acceptance-summary.json`。最終状態・保全確認: `pi-final-audit.json`。
- pi-node作業領域: `/home/maakie/vlb-permanent-lfuCi1JL/`
- 配置: `permanent-install-yfz_impr/`
- 受入れ前半: `permanent-accept-gwqnfg32/`
- 復旧続行: `permanent-accept-q4rc1q5j/`
- 最終続行: `permanent-accept-sqqwrpr7/`
- [検証driver](../../scripts/local-ban-permanent-systemd-check.py)と、承認時のbundle/hashはlocal証拠に保存。
