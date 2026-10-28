# AWS一台試験：隔離実XDP合格、ENAのMTU条件でNIC装着を停止

2026-10-26 JST。最終ゴールは小規模公開サイトでの限定試験導入、今回のマイルストーンは一台のLinux VMへの適合確認。本人が[一台の具体案](2026-10-26-AWS-MFA-CLI-READONLY-AND-SINGLE-PLAN.md)の作成・費用・期限付き終了・撤去を承認した範囲で実施した。

**AWSの一回のVM作成は成功し、同一成果物による隔離実XDPの全32確認が通った。実NICのENAはMTU 9001とXDPの組合せを拒否し、kernelログで上限3498を明示した。MTUや装着方式を変えて再試行せず、新設物を撤去。公開受入れは未達、標準MTUのままの当該構成は採用しない。**

## 実施した構成と時間

| 項目 | 記録 |
| --- | --- |
| account／region | 既存account末尾5221、us-east-1／us-east-1a |
| VM | `i-07b9350c18b4150a1`、t3.small一台、RunInstances一回。ClientToken固定、SDKの自動再試行なし |
| image | Canonical `ami-0d6dd9e8265d48906`、Ubuntu 24.04、`ubuntu-noble-24.04-amd64-server-20261023` |
| 作成要求 | Standard CPU credits、暗号化20GiB gp3・DeleteOnTermination、ephemeral IPv4。追加disk・Elastic IP・instance profileなし |
| ゲストで確認 | `7.0.0-1014-aws`、x86_64、ens5／ena、MTU 9001、private IPv4 `10.77.26.14`。lockdown表示は`[none] integrity confidentiality`。Shielded／Secure Bootとの同等性を主張しない |
| 公開範囲 | 承認したMacの固定値一件/32→TCP22だけ。18443／80／443／IPv6の公開なし。VMにAWS credential・SSH秘密鍵を配置しない |
| 実行開始T0 | 21:45:53.046 JST。準備・承認待ち中は開始していない |
| 期限設定 | T0＋45分、22:30:53 JSTの一回限りSchedulerを読み戻して確認。専用group・tag限定TerminateInstances role、対象instance ID固定 |
| 撤去完了 | 21:52:01.454 JST、T0から約6分8秒。21:53:18 JSTからの別照会でも残存なしを確認 |
| 費用 | 承認上限US$0.20／60分、税前計画額US$0.08。追加VM・公開B・有料supportなし。確定請求は未照合で、$0とは主張しない |

CloudShellの既存rootセッションによる短期実行を使い、新しいrootキーや既存CLIユーザーの作成権限を追加しなかった。本人確認、専用パスへのスクリプト転送ハッシュ、オフラインの単一VM・IMDSv2・Standard credits等の条件を確認してから実行した。終了後はシェルをexitし、今回作ったブラウザー作業タブだけを閉じた。CloudShellの今回の証拠は保全した。

Schedulerは新設roleの反映直後に二回、AssumeRoleのValidationExceptionを返し、同じscheduleを三回目に設定できた。この履歴も残している。VM作成要求の再送はない。Schedulerの設定・読取りは確認したが、手動撤去が先に完了したため、45分時点の実発火・終了の障害注入は未実施。

## 同一成果物と隔離試験

| ファイル | SHA-256 |
| --- | --- |
| veil-local-ban | `e15789d25d24345a4fdba5b8f0ae0bad5199a4878fcfeab38153e4827d784890` |
| veil_xdp_kern.o | `2ee94e0b78ced2bba236f2751a0332a5a98db9f63404381879743ed49d63db2f` |
| 既存host非依存driver | `fcf851556d39f399eaab2f09ae6187eac55561b3e69a882a251ebfbb9c51de00` |

製品コード・WAL・TTL・安全条件を変更していない。既存driverの新規PrivateNetwork runnerを使い、専用profileに承認済みの`RuntimeMaxSec=180`を明示。runnerは240秒、有限TTL・256件上限・分割掃除を維持した。製品等は約10.7MBに圧縮して転送し、guestで各hashとCLIのABI起動を確認した。新VM以外のホストには接続・設定変更していない。

隔離namespace／vethの試験はexit 0、32/32確認、runner実行時間77.520秒。主な確認は次のとおり。

- 証明書を明示信頼した正常HTTPSと対照port、一覧0、previewの非変更性。
- previewから一件BAN、対象通信の遮断とkernel counter、対照port継続、明示解除。
- 古いrevision、解除済み操作、保護IP、TTL=0の拒否。
- 短い有限TTLとdaemonのSIGSTOP中のkernel失効、掃除の検査・試行数が一回16以下。
- 256件の完全一覧、257件目をpreview・直接適用とも拒否し、既存entryを退避・追い出ししないこと。
- 通常restartで256件が消失、新instance、旧instanceのBAN／解除を拒否。
- SIGKILL後の自動再起動とBAN0、再起動後も全threadが非root・capabilityなしであること。
- 明示停止後の所有XDP非装着、通信復帰、DynamicUser解放、server回収。

[隔離結果と各CLI出力](../../artifacts/aws-one-live-ng5ti4th/guest-evidence/result.json)、[runner出力](../../artifacts/aws-one-live-ng5ti4th/isolated.stdout)、[終了コードと資源量](../../artifacts/aws-one-live-ng5ti4th/isolated.stderr)。これは外部WANやブラウザー既定の証明書信頼の受入れではない。

## 仮想NICで特定できた停止原因

ens5への装着前に既存XDPなし・TC filterなしを確認。保護tupleはprivate IPv4のTCP18443、BANは行わず、既存loaderの装着方法をそのまま使用した。

12:50:41 UTCの製品ログは`bpf_link_create`のEINVAL、同時刻のkernelログは次の内容だった。

```text
ena 0000:00:05.0 ens5: Failed to set xdp program, the current MTU (9001) is larger than the maximum allowed MTU (3498) while xdp is on
```

**今回の装着失敗は、実測環境のENA／MTU条件に絞れた。** 隔離vethでは同じ製品が動いており、eBPFの一律禁止や署名検証・GCのタイムアウトが今回の停止原因だという証拠はない。

[Amazon公式driver資料](https://github.com/amzn/amzn-drivers/blob/master/kernel/linux/ena/README.rst#xdp-support)も、multi-bufferを用いない場合のMTU制約を説明している。ただし掲載版の数値を今回のguestへ流用しない。今回の上限3498は実際のkernelログの値で、すべてのENA／kernelに共通する定数ではない。multi-bufferには別のprogram条件があり、今回それを追加して試してはいない。

MTU・offload・queue・route・製品のmode指定を変えず、一回の装着失敗で停止した。失敗unitのMainPID0、XDP非装着、routeとTC filterの前後一致、新規SSH成功を確認。**装着中の新規SSHは、装着自体が失敗したため未検証**。失敗後の非装着を、成功した装着・解除の証拠へ読み替えない。

[NICの前後状態](../../artifacts/aws-one-live-ng5ti4th/guest-evidence/nic.json)、[製品とkernelの診断ログ](../../artifacts/aws-one-live-ng5ti4th/nic-diagnostic.stdout)、[Macからの接続結果](../../artifacts/aws-one-live-ng5ti4th/nic-client-result.json)。guestの最終取得時NIC累計は受信約11.36MB・送信約0.268MBで、作業上限100MiBを超える転送を行っていない。これはAWS請求の転送量照合ではない。

## 撤去と保全

製品の結果・journal・NIC状態をMacへ退避してから、作成時IDと固有run tagを照合して撤去した。試験TLS秘密鍵やAWS credentialは退避対象に含めなかった。

- instanceは`terminated`、今回tagのboot volume0。EC2に終了済みinstanceの表示が残ることと、稼働・課金対象が残ることを混同しない。
- 今回の専用SG、route association／table、IGW、subnet、VPCを撤去。
- 対象instance終了後にschedule、専用execution roleのinline policyとrole、専用schedule groupを撤去。
- VPC・subnet・SG・route table・IGW・volume・予約addressの七区分のID集合が事前と一致。さらに別照会で今回tagの六区分0、role／group不存在、instance終了を確認した。全service・全region・全IAM属性や請求の照合ではない。

[全API履歴・作成要求の応答・独立した最終照合](../../artifacts/aws-one-live-ng5ti4th/aws-one-ng5ti4th-final.json)、[撤去後の画面](../../artifacts/aws-one-live-ng5ti4th/cloudshell-cleanup.png)。既存root MFA・CLI設定、Pi／hp／router／Macネットワークは変更していない。別プロダクトの既存IAMポリシーに触れていない。

## 採否と残り

| 判断対象 | 結論 |
| --- | --- |
| AWSでVMを作成し、今回の製品を動かせるか | 作成・SSH・隔離実XDPまで確認できた |
| 現在のimage既定MTUのままENAへ配備できるか | 不採用。MTU 9001でNIC装着が拒否される |
| AWS自体を候補から外すべきか | その根拠はない。具体的なNIC条件を満たす構成なら次の適合確認候補 |
| 公開サイトへの導入 | HOLD。仮想NICの成功した装着・解除、公開六条件は未確認 |

最小の次案は、**新しい専用試験VMに限ってMTU1500を一時指定し、同じ製品でNIC装着・新規SSH・停止解除・MTU復元を一回確認すること**。製品の独自暗号・BFT・永続化変更やmulti-buffer対応を追加する案ではない。MTU低下はパケット分割・帯域効率・一時的な通信影響を伴い得るため、変更値と復旧・費用を具体化して改めて確認し、今回の承認を延長しない。MTU1500なら成功するという実測はまだない。

残りは、(1) 対応するNIC条件での適合確認、(2) 別承認による公開経路の六条件、(3) 撤去・本人の操作確認・限定導入判定。環境と承認が揃えば次のNIC確認は最大60分枠、公開受入れは従来の2〜4時間案だが、未実測の工程見積りであり追加費用・時間の承認ではない。今回の一台試験と撤去は終了、製品の最終ゴールは未達。[最終受入れ条件](../local-ban/DELIVERY_GATE.md)・[試験履歴](../local-ban/TEST_HISTORY.md)。
