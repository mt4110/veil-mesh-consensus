# ホスト再起動を跨ぐローカル一時BANの隔離受入れ

2026-10-25 JST。[計画](2026-10-25-LOCAL-BAN-BOOT-PLAN.md)に対して、設定配置・一度の正常再起動・最後のdisableによる所有symlink除去まで、ユーザーの明示承認後に実施した。

**namespace復元→systemd自動起動→新instance/BAN 0を実際のホスト再起動で確認した。新旧4 unitはすべて停止・disabled。**
ただし、当初のhost監査条件「mainnetの同一container ID」は未達。Cargo経由の再ビルドも、再起動後に固定Nixパスが不在となり未確認。この2点を成功へ書き換えず、下に区別する。

最終ゴールは小規模公開サイトで安全に試用できる無償OSS。今回のマイルストーンは、隔離環境でのboot時復旧と揮発BANの契約確認。**この機能確認は完了したが、本番導入・host全体の無影響・再現可能な再ビルドまで合格したという判定ではない。**

## 実機で確認した動作

| 対象 | 証拠・結果 |
| --- | --- |
| ホスト | SSH名pi-node、実名maakie-lab、Linux x86_64。kernelは前後とも7.0.0-38-generic |
| 再起動 | 正常なsystemctl rebootを一回だけ要求。新boot ID、SSH切断・復帰を観測。SSH復帰を約72.4秒で観測（約5秒間隔の観測であり、kernelの起動所要時間そのものではない） |
| namespace | 今回bootのjournalでhelperのrestored/changed=trueを確認。固定MAC/IP/MTU/peer構成を復元。停止後のread-only inspectもverified_existing/changed=false |
| 自動起動 | postbootではstart命令を送る前にactive/enableを確認。namespace unitの完了がdaemon開始より先。daemonの実際のnetwork namespace inodeも復元先と一致し、hostの初期namespaceとは異なる |
| 再起動前の容量 | 256件を保持、257件目はCAPACITY。実通信元のBANを追い出さず、対象外portは通信継続 |
| 揮発契約 | 再起動後は新instance、capacity256/occupied0。旧instanceを使う管理要求はINSTANCE_CONFLICTで拒否 |
| 自然失効との区別 | 再起動要求時に旧BANの残TTLは約3478秒。新instance/BAN 0を確認し終えた最終監査までのclient側経過は約178秒未満。自然失効待ちを再起動による消失と取り違えていない |
| 再起動後の通信 | TLSのIP SANとrun固有bodyを検証。18443/TCPのみBAN、18444/TCPは継続。TTL3秒のkernel失効、分割掃除、個別解除も成功 |
| 稼働権限 | 再起動前後の全threadでUID979、補助group979のみ、CapEff/Prm/Inh/Amb=0、NoNewPrivs=1 |
| 最終状態 | boot/namespace/full/testの4 unitがinactive/disabled、MainPID0。XDP非装着、namespace内processなし、試験listenerなし、専用UID979/980のprocessなし |

再起動前9箇所、再起動後14箇所のdriver確認が成功した。これは上記動作の証拠の索引であり、項目数自体を安全性の証明にしない。
製品Rust/BFT/WAL・sync_all・TTL上限・時間制限は変更していない。再起動前の入力TTL3600秒は、既存上限86400秒内の有限値。256件上限と再起動時の揮発契約を維持した。

## 配置と実装

- [固定namespace helper](../../scripts/local-ban-namespace.py)と[試験用2 unit/config](../../deploy/systemd/pi-boot-test/)を新設した。helperは既知の既存構成なら検証のみ、全体が不存在なら固定構成を作成する。衝突・部分構成・未知のmount/interface/process/XDPは削除や修復せず拒否する。
- root helperのCAP_SYS_ADMIN/CAP_NET_ADMINはnamespace作成のoneshotだけ。通常daemonは従来の起動profileを使用し、起動後に権限を破棄する。helperのmountをhostから見せるための制約は計画どおり。
- [試験driver](../../scripts/local-ban-boot-check.py)はinstall/preboot/postbootを分け、各段階の開始markerをO_EXCLで作る。rebootそのものは外部の一回限りの操作として別記録を先に作り、SSH切断による曖昧さで再送しない。
- 初回installは、systemctl showが空のDropInPathsを省略することをdriverが想定しておらず、配置とsystemd-analyze verifyの成功後、結果集計でKeyErrorになった。**このinstall-resultはcomplete=falseで残している。** その時点ではdaemon起動も再起動もしていない。
- 空値をgetで扱う最小修正を行った。元driver、manifest、失敗記録は上書きせず、root所有0600・単一linkのdriver改訂記録で旧/新SHAを対応させた。実配置の全hash・正しいFragmentPath・override不在・inactive/disabledを別途再検査してから、修正版でpreboot/postbootを実行した。元の配置検査や期限を省略していない。

## 回帰と保全

- helperのmock検査12件とnative read-only検査は準備時に成功。driverの権限・未知symlink保全等5件、改訂記録のroot所有/mode/link数/旧新digest検査7件も確認した。
- Linux library回帰は**24 passed / 0 failed / 1 ignored**。既存の検証済みtest binaryを直接実行した。ignoredは従来の明示的な初期化SIGKILL用fixture。
- Cargo経由の試験起動は、`/nix/store/gmgfpzc0w2pgby0lmq9r88m39albiyfw-cargo-1.98.1/bin/cargo`のFileNotFoundErrorで開始できなかった。再起動後は/nix/storeが存在せず、nix-daemon unitも存在しない。storeが見えなくなった原因とbacking dataの所在は未特定。恒久配置やmount変更、package再導入は承認範囲外なので実施していない。**新規ビルドが成功したとは報告しない。**
- 既存のtest/full両fixtureは、前後のmode/UID/GID/inode/device/size/type・通常fileのhashが一致。元のunit/drop-in/configも保全した。旧primary/recoveryへ障害注入や修復をしていない。
- passwd/groupの既存entry hashは保持。アカウント作成・変更は行わず、既存locked/nologinのUID979を新unitの新primaryにだけ使用した。shadow/gshadow本文は取得していない。
- SSH/Docker/timesyncd/UFW/NetworkManager/pi-mainnetのunit本体/drop-inのhash、LoadState/ActiveState/UnitFileStateは一致した。再起動でPIDは変わるため、PID一致とは判定していない。
- 物理NICの名前/MAC/MTU、IPv4設定、default routeも一致。namespace/vethは計画どおり新しいkernel objectとなり、MAC/IP等の構成を復元した。
- local/nativeの製品関連68ファイルは作業前後で一致。BFT全体の回帰は今回実行していない。

## mainnetの監査条件について

**元の「same container IDかつrunning」のチェックはfalseのまま保存した。** 前後ともrunning、restart policy unless-stopped、StopTimeout120秒だが、container IDは変わった。

読取り調査では、変更していない既存pi-mainnet.serviceがExecStopでdocker compose down、ExecStartでupを実行することを確認した。volume削除flagは付いていない。今回のID変更は、その既存のコンテナ再作成方式と整合する。今回の試験からDockerのdown/rm等を手動実行していない。
現在のコンテナは、既存の`/home/maakie/pi-node/docker_volumes/resync-20261014/{stellar,supervisor_logs,history}`をbind mountしており、これらのdirectoryは存在する。

ただし、**再起動前の実mount一覧を採っておらず、bind先の前後完全照合やアプリの論理的整合性・同期完了を証明できていない。** 同一container IDを条件にする前に既存のcompose lifecycleとmountを確認すべきだった。元のfalseを成功へ書き換えず、既存方式と整合する再作成という追加観測を別に記録した。

## 残置と次の判断

新設したのは、計画に記した/opt/veil-local-ban-pi-boot、/var/lib/veil-local-ban-pi-boot/primary、2 unit、private staging。primaryには停止後のsocket-ownerだけが残る。アカウント新設はない。
enableで作った所有確認済みsymlink一本はdisableで除去済み。namespace unitは直接enableしていない。namespace/veth、unit/config/helper、fixture、証拠は削除せず残した。

**今回の起動・復旧契約は実証できた。** 本番導入には、実サイトの入口・保護IP・既存防御を決めた受入れが引き続き必要。IPv6、CDN越しのclient識別、攻撃検知の精度、帯域を埋める攻撃への耐性は今回保証しない。
開発用Nix環境の再起動後の再構成は未解決。配布済みdaemon binaryの自動起動は成功したが、開発toolchainの再現性まで完了したとは扱わない。
試験後はdisabledなので、以後のbootでこのnamespaceが自動復元される本番運用にはしていない。追加のrebootやNix/他サービス設定変更は実施していない。

上記はこのboot試験時点の記録。その後の[ビルド復旧調査](2026-10-25-NIX-BUILD-RECOVERY.md)で、既存runtime copyの残存と一時bind mountの消失を確認した。hostの恒久設定を変えず、一時build namespaceで新規コンパイルとCargo経由の回帰に成功した。元のCargo起動失敗記録は保全する。

## 証拠

- local: `artifacts/boot-namespace-02j1usy3/`
- [集約](../../artifacts/boot-namespace-02j1usy3/acceptance-summary.json)、[元の最終監査条件](../../artifacts/boot-namespace-02j1usy3/final-checks.json)、[監査詳細](../../artifacts/boot-namespace-02j1usy3/final-audit.json)
- preboot-result.json/postboot-result.json、reboot-request.json/reboot-watch.json、namespace-boot-journal.log/daemon-boot-journal.log。
- install-result.json（失敗を保全）、installation-verified-after-driver-fix.json、driver-revision.json、library-direct-regression.log/library-direct-binary.json、cargo-launch-failure.json、mainnet-compose-assessment.json/host-followup.json/last-state.txt。
- remote: `/home/maakie/vlb-boot-f4d85_9u/`。秘密key本文や環境変数は報告へ取り込んでいない。
