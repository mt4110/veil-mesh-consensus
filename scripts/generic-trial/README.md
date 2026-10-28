# 明示Generic専用の試験package準備

これは[GCP一台の具体案](../../docs/local-ban/GCP_GENERIC_SINGLE_VM_PLAN_20261027.md)向けの試験用toolです。製品の通常運用CLIや自動防御ではありません。VM作成・公開・削除APIを実行するコードは含めず、作成応答ID・boot・ifindexを確認できる専用VMだけを対象にします。実NIC操作は確定値の承認後に限ります。

## 内容と権限

| file | 役割 |
| --- | --- |
| package.py | 全assetを外部catalog SHAへ固定し、新規archiveだけを作成・検証。旧helper／archiveは使い回さない |
| access-check.py | root所有・mode・hash・非root DACと実--help／serve --helpを検査。serveやBPF loadはしない |
| native-regression | 同じsource・toolchainで生成したRust library回帰のx86_64実行ファイル。全51件と従来のignore一件をそのまま確認し、失敗を別のPython判定へ置き換えない |
| features.py | feature全体のbounded GET、loのNETLINK GET smoke、off／restoreの純粋な引数計画。SETを実行する入口は持たない |
| driver.py | 接続のない新規namespaceで機能的BANを検証。全起動が明示Generic。TTL・待機・capacity基準は旧driverから維持 |
| nic-check.py | package・GCE metadataのproject／zone／name／作成応答ID・bootを照合。記録後に二featureの必要SET、空BANの固定二case、復元を実施する専用orchestrator |
| profile.service | 既存sandbox・180秒・起動／停止10秒を維持。Restart=no。常設配置やenableを行うtemplateではない |
| test_offline.py | 保全したfixtureとmockだけで拒否・引数・復元・結果不明／再送拒否・package改竄を検証。NIC SETやGCP APIを呼ばない |

scope.jsonとx86_64 binary／BPF objectは、別の新規artifact領域で用意する。catalogとmanifestは別のhashであり、manifestの自己整合だけを外部の信頼根拠にしない。scopeのcloud_execution_approvedは生成時点の記録で、本人承認やGCE所有照合の代わりとなる実行switchではない。

## 課金前の使い方

1. current source／lock／toolchainを固定し、GLIBC要求版がUbuntu24.04と互換なx86_64版を別targetへbuildする。
2. 新規asset dirへ九asset（native-regressionを含む）をcopyし、path／SHA-256を列挙したversion2 catalogを作る。元のdebug targetはhardlinkのことがあるので、単一linkのcopyを使う。
3. 次を実行し、archive SHA・manifest SHA・全asset SHAを保存する。catalogを変更したら、そのhashも別に確定する。

```sh
python3 -B scripts/generic-trial/package.py build \
  NEW_ARCHIVE NEW_CATALOG CATALOG_SHA256
python3 -B scripts/generic-trial/package.py verify \
  NEW_ARCHIVE NEW_CATALOG CATALOG_SHA256
VEIL_GENERIC_EVIDENCE=NEW_EXISTING_PRIVATE_DIR \
  python3 -B scripts/generic-trial/test_offline.py
```

QEMU実行はx86_64ネイティブkernel経路の合格ではない。QEMUでGETQDISC／GETTFILTERがEOPNOTSUPPとなる記録を残し、製品のguardをskip／緩和しない。native GCPで下記の読取りgateを先に実行する。

## 将来の承認済み新VMだけで行う順序

以下は操作例であり、文書を読んだこと自体を実行承認としない。転送・展開前にarchive hashと全memberの属性を検証し、未使用のroot所有0755の`/run/veil-gcp-generic-20261027/`へ配置する。全ファイルはroot所有、binary0755・他0644、単一link。旧dir・既存unit・不明な記録を削除・修復しない。

1. access-checkのservice-checkで、15秒のcapabilityなしDynamicUser unitによる実起動と、同梱したnative-regressionによる51件の全library回帰を確認する。実kernel GETを必要とするため、access専用unitに限りAF_UNIXに加えAF_NETLINKを許可する。CAP_NET_ADMINは与えず、PrivateNetworkを維持する。hostに常設unitを置かない。
2. **NIC設定・BPF loadより前に**、通常の非rootユーザーでfeatures.pyのroute-get-smokeを実行する。loに対するGETLINK／GETQDISC／GETTFILTERだけ。失敗すればGCP trialを止め撤去する。features.py get ens4でも全active／wantedを取得できることを確認する。
3. 240秒のPrivateNetwork runnerでdriver.pyを起動。32の既存確認を維持し、SIGKILL後の自動restartを明示restartへ適合した。新規に「明示restart前のlink解放」も検証するので、現在は33確認。失敗時はその場で修正せず、実NICへ進まない。
4. runner終了・全check合格・cleanup_ok・server_reaped・所有namespace回収・unit停止を確認する。
5. nic-check initに、**Macで取得した今回の作成応答ID**と最初の新設操作直前のT0（Unix秒）を渡す。metadataだけから自分のIDを自己承認しない。B0を二回読取り、同一boot／netns／ifindex・VirtIO・MTU1460・10.77.28.10・空XDP／TCを要求する。
6. prepareは必要な二featureを一つのethtool引数列でoffへ設定する。全対象が変更可能か先に検査し、SET前に一回限りのattemptを保存する。失敗・結果不明なら同じ操作を再送しない。
7. start-normal → **Macから別の新規SSH** → stop-normal → 新規SSHとB1不変、次にstart-kill → 新規SSH → stop-kill → 新規SSHとB1不変。NICではBANしない。新規SSH失敗を既存の保持接続の応答で代用しない。外側の実行担当は各失敗で後続caseを止める。
8. restoreは全unit停止・XDP不在・同じidentity・無関係なfeature不変を検査し、変更したwantedだけをB0へ戻す。activeもB0に一致しなければ失敗。未知の変化を全SETで上書きしない。
9. 新規SSH・復元・停止と証拠退避を確認し、承認した期限・所有照合で今回resourceを撤去する。削除や外側のSSH判定はこのpackageの機能ではなく、承認したMac側実行手順の責任である。

```sh
TRIAL_ROOT=/run/veil-gcp-generic-20261027
sudo /usr/bin/python3 -I -B "$TRIAL_ROOT/access-check.py" service-check "$TRIAL_ROOT" MANIFEST_SHA256
/usr/bin/python3 -I -B "$TRIAL_ROOT/features.py" route-get-smoke
/usr/bin/python3 -I -B "$TRIAL_ROOT/features.py" get ens4
sudo /usr/bin/python3 -I -B "$TRIAL_ROOT/nic-check.py" init MANIFEST_SHA256 --vm-id CREATE_RESPONSE_ID --t0 T0_UNIX_SECONDS
```

prepare／start-normal／stop-normal／start-kill／stop-kill／restoreは同じmanifest SHAを必須とする。正常caseは25分、kill caseは32分までの開始、40分以降は新規case禁止。所有不明・record破損・時間切れ・一部失敗は成功扱いにしない。NIC SET後にSSHを失った場合、別NICへの迂回やhost再起動ではなく、Macのcontrol planeから所有確認済みの今回VMを削除する。

helperがunitを止めても、元のoffload設定まで自動的に戻るわけではない。復元はrestoreの別段階として記録する。既存のBFT・WAL・製品BOOTTIME絶対期限・256件上限・restartでBAN0の契約は変更しない。
