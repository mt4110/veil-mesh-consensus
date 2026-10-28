# nftables owner機能だけの隔離確認案

作成: 2026-10-27 JST。**後続の本人承認で一回実施し、owner回収・終了・保全照合まで完了しました。** [実施結果と原出力](../review/2026-10-27-NFT-OWNER-ISOLATED-ACCEPTANCE.md)。以下は実行前の固定案として保全するもので、再実行の承認ではありません。試験unitは停止・非enabled、fixtureは保全済みです。

その後の[期限の採否判断](../review/2026-10-27-NFT-BOOTTIME-ADOPTION-DECISION.md)で、現行契約を維持する標準nftablesへの移植は不採用と確定しました。この計画を追加試験の指示として再開しません。

**実行前の修正記録:** PID 1 namespaceの直接参照をroot所有の事前参照ファイルへ置き換えました。固定path・所有者・mode・形式・同じbootを検証し、既存の権限／隔離／時間制限は維持。既存15件を含む24件のoffline検証が成功しました。[読取り時の問題](../review/2026-10-27-NFT-OWNER-READONLY-PREFLIGHT.md)・[修正と検証結果](../review/2026-10-27-NFT-OWNER-HOST-REFERENCE.md)。その後の配置・実機試験は冒頭の別報告に記録しています。

最終ゴールは、小規模公開サイトで安全に運用できる手動一時BANです。現在のマイルストーンは、nftables候補の権限境界と停止復旧を評価すること。今回のゴールは、**現行のCLOCK_BOOTTIME絶対期限を維持し、BAN適用を保留したまま、owner機能だけの隔離確認案とscriptを準備すること**です。

## 1. 何を判断するか

専用Netlink socketが所有する空のtableについて、socketを閉じた場合と、そのsocketを持つ子processをSIGKILLした場合の回収を確認します。別socketの対照tableが残ることを、第三の観測socketから確認します。

**chain・rule・set・hook・BANは作りません。packet試験、TTL試験、256件試験、公開経路試験は含みません。** 空のtableの成功を、遮断解除・期限保証・本番brokerの最小権限の証明へ読み替えません。

Linux v6.8の一次資料では、owner tableの回収はNetlink socketのrelease通知と所有port IDに結び付いています。table一覧には所有port IDも含まれます。[回収実装](https://github.com/torvalds/linux/blob/v6.8/net/netfilter/nf_tables_api.c#L10681)・[table応答](https://github.com/torvalds/linux/blob/v6.8/net/netfilter/nf_tables_api.c#L898)。これは対象hostでの実測ではありません。

## 2. 対象と影響

| 項目 | 固定案 |
| --- | --- |
| host | hp-ubuntu。Pi・cloud・router・Macのnetwork設定へ操作しない |
| 新規一時unit | `veil-nft-owner-probe-20261027a.service`。既存同名unitがあれば停止して再利用せず、試験を中止 |
| 新規配置先 | `/run/veil-nft-owner-probe-20261027a/`、root所有0700。既存なら上書き・削除せず中止 |
| script | 同directoryの`nft-owner-probe.py`。読取り専用配置、元scriptとSHA-256を照合 |
| 事前参照 | 同directoryの`host-reference.json`、root所有0400・通常ファイル・単一リンク・4KiB以下 |
| 証拠 | 同directoryの`evidence/`、root所有0700。`result.json`は排他的新規作成。既存証拠を上書きしない |
| network | systemdの`PrivateNetwork=yes`による新規namespace。loのみ。veth・物理NIC・IP付与・route変更なし |
| kernel変更 | そのnamespace内の空table最大2個、`vno_guard`と`vno_owned`。family inet、ownerのみ、persistなし |
| 権限 | 試験fixtureはroot、capabilityはCAP_NET_ADMINだけ。namespace変更のsyscallを制限。実製品の権限分離ではない |
| 時間・資源 | unit最大60秒、停止猶予5秒、Restart=no、TasksMax=8、MemoryMax=64M |
| 常設変更 | account・常設unit・enable・package導入・module設定・host再起動なし |

systemdの隔離・制限は[公式の実行環境仕様](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html)を基にしています。後続の実行ではunit引数を変更せず受理され、probeの境界検査も成功しました。非対応propertyを外して試験を続けるfallbackは設けていません。

共有kernelのCPU・メモリ・Netfilter処理は利用します。PrivateNetworkは別VMではなく、kernelの不具合や資源競合まで無影響と保証するものではありません。60秒と停止猶予も、kernel内の停止を必ず解消する保証ではありません。

## 3. 配置・実行前の確認

この節は将来の承認後の手順案です。今回の準備ではSSHもroot操作も行っていません。

1. 新規SSH、hostのkernel／systemd／Python、既存のinterface・address・route・firewall・関連service状態を読取り確認し、private evidenceへ保存する。host firewallは保存用の読取りだけとし、restore／flushしない。
2. 上記unitと配置先が未使用であることを確認する。存在する場合は所有不明物として保全し、中止する。古いfixtureを流用しない。
3. `nf_tables`が既に利用可能なことを確認する。未ロードなら中止し、modprobe・導入・再起動で継続しない。
4. 承認された新規directoryだけにscript、host-reference.json、証拠領域を作る。directoryはroot0700、scriptはroot所有・非書込み、参照ファイルはroot0400とし、証拠領域だけ書込み可能とする。参照内容は配置時にrootで読み取ったPID 1のnet／pid／user namespaceのdevice番号・inode番号とboot ID。準備済みの古い観測値や手入力の例を流用せず、排他的新規作成してhashを記録する。転送前後のscript hashも照合する。
5. 次のオフライン出力の`review_only_systemd_run_argv`を確認する。これは引数配列の表示で、実行機能ではない。承認後の実行担当が固定内容と配置hashを照合して一度だけ実行する。

```sh
python3 -I -B scripts/nft-owner-probe.py --offline-plan
```

出力するunit案は、PrivateNetwork・CAP_NET_ADMINのみ・NoNewPrivileges・RestrictNamespaces・AF_UNIX/AF_NETLINKのみ、ProtectSystem=strict、ProtectHome、PrivateDevices等を指定します。書込み先は新規evidence directoryだけです。RuntimeDirectoryによる証拠の自動除去は設定しません。RestrictNamespacesは、managerが用意したnamespaceから試験process自身が別namespaceへ移ることを制限するための指定です。

script自身もkernel操作前に、Linux／root／PID・user namespace／hostと異なるnet namespace／専用cgroup／capability／NoNewPrivileges／loのみを確認します。PID 1へ直接アクセスせず、root所有の事前参照と`/proc/self/ns/*`を比較します。全familyのtable一覧が空でなければ作成しません。host namespace、既存table、余分な権限・NICは拒否します。条件を緩めるfallbackはありません。

### 参照ファイルの固定形式

| field | 許可する値 |
| --- | --- |
| version | 整数1。boolや他versionは拒否 |
| boot_id | 配置時のboot IDの小文字UUID。実行時のboot IDとの完全一致が必須 |
| init_netns | PID 1のnet namespaceの`[st_dev, st_ino]`、正のu64整数2個 |
| init_pidns | PID 1のpid namespaceの同じ形式 |
| init_userns | PID 1のuser namespaceの同じ形式 |

余分・欠落・重複field、不正UTF-8、破損JSON、読み取り中にmetadataやサイズが変わった参照は拒否します。固定directoryをFDで開き、そのFDを基準に固定basenameだけをO_NOFOLLOWで開きます。runtime自体のsymlink、参照のsymlink・hardlink複製・FIFO・directoryも拒否します。必要な識別値を取得するrootによる配置前の読取りは、権限を制限したprobeとは別の準備処理です。probeにCAP_SYS_PTRACEやCAP_SYS_ADMINは追加しません。

参照の信頼性は、正しいrootの採取・配置、rootだけが管理する`/run`と新規directory、unitの読取り専用制限に依存します。内容だけで採取元を暗号学的に証明する機能ではなく、root侵害への防御ではありません。後続で実OSの配置・読取りとnamespace／capability等の明示検査は成功しましたが、全systemd制限への突破試験や製品brokerのOS認証は対象外です。

## 4. 一回の試験内容

| 段階 | 操作と合格条件 |
| --- | --- |
| 初期 | 観測socket Oの完全な一覧応答で、全familyのtableが0個 |
| 対照作成 | socket Gでownerの空table `vno_guard`を一回だけ作成 |
| socket終了 | fork後に子だけが新規socket Cを作り、`vno_owned`を所有。Oで2個の名前・family・owner・flags・use=0を確認。子がCだけを閉じ、生存したまま待つ間に、Oでowned不存在・guard存続を確認。その後に子を正常終了 |
| SIGKILL | 新しい子が新しいsocketとowned tableを作成。Oで2個を確認後、その未回収の自分の子だけをSIGKILL。wait結果と、Oからowned不存在・guard存続を確認 |
| 最終 | Gを閉じ、Oの完全な一覧応答で再びtable0個を確認。result.jsonを保存し、unit終了・非enabledと新規SSH・既存設定保全を独立確認 |

一覧はkernel送信元・sequence・multipart終端・error・generation整合・受信サイズ／件数上限を確認します。ACK不足、権限不足、timeout、途中切れ、dump中断を「不存在」に変換しません。作成要求は再送しません。通常応答待ちは一操作2秒、子準備待ちは最大4秒です。これらは試験の打切り条件であり、回収時間の製品保証ではありません。

子は観測／対照socketの継承FDを閉じ、所有socketをfork後に作ります。exec継承も無効にします。close試験とSIGKILL試験を混同しないため、close試験では所有processの生存を非回収のwaitidで確認します。

## 5. 中止・復旧

- 準備条件不成立、未知table、owner不一致、ACK不明、回収未確認で中止。変更要求を再送せず、同名tableのdelete／flushも実行しない。
- scriptが作った子とsocketだけを終了する。障害時の外側の操作は、所有を照合した今回のunitへの`systemctl stop veil-nft-owner-probe-20261027a.service`だけとする。
- unit終了で、そのunitのprocess・socket・PrivateNetworkの参照が解放されることを確認する。終了／回収を観測できなければ「復旧済み」としない。host firewallの全消去・既存service停止・host再起動で解消しようとしない。
- hostの新規SSH・interface／address／route／firewallと既存serviceを事前状態と照合する。第三者の同時変更があれば勝手に戻さず、差分と不明点を記録する。
- `/run`の新規script・証拠は削除せず残す。host再起動で失われるため、終了時にhashと共にMacのprivate artifactsへ退避する。これは証拠コピーであり、再起動の依頼ではない。

## 6. 終了判定と次へ進まない条件

**この準備タスクの完了:** script、模擬応答によるoffline検証、固定unit案、復旧手順、READMEから辿れる履歴を揃える。実機成功は完了条件に含めない。

**将来のowner確認の完了:** 一回の隔離実行で、socket単独close／SIGKILLの両方、対照存続、最終table0、unit終了、host保全の証拠が揃う。失敗なら原因と非対応範囲を記録して止め、試行を自動追加しない。

**owner確認が成功しても、BAN実適用はBLOCKEDのまま。** 絶対期限、実peer認証、権限分離、broker停止連動、実packet、256件・期限・解除・restart・公開受入れは別です。現行期限と同等の方法が見つからなければ、nft移植を採用せず、現行XDPを条件付き選択肢として保全します。一般的な相対TTLを許容する別製品契約は、今回採用していません。

[実施した準備と検証結果](../review/2026-10-27-NFT-OWNER-PROBE-PREPARATION.md)・[期限と権限境界の設計](NFT_BOUNDARY_DESIGN.md)・[試験履歴](TEST_HISTORY.md)。
