# DigitalOcean短期VM試験の構成・費用・復旧案

2026-10-26。本人は新規DigitalOcean案の具体化を選択した。**VM作成・課金・公開設定は未承認で、未実施。** [製品のゴールと受入れ条件](DELIVERY_GATE.md)を満たす候補構成を以下へ固定する。

**後続の方針変更:** 契約を増やさない本人の希望により、現行案は[既存GCPのveil-x-tests](GCP_TRIAL_PLAN.md)へ切り替えた。以下は旧比較案として保全し、DigitalOceanのaccount準備・作成へ進めない。

## 作成候補

| 項目 | 案・確認状況 |
| --- | --- |
| 保護対象VM | 新規`veil-ban-target-test`一台 |
| 試験元VM | 新規`veil-ban-client-test`一台。稼働中の公開IPv4を固定して使い、自動で再作成・追従しない |
| OS | Ubuntu 26.04 LTS x64。公式image一覧に`ubuntu-26-04-x64`あり。実kernel／NIC／XDPの対応は作成後の適合確認が必要 |
| 各size | Basic Regular、1 vCPU・2GiB RAM・50GiB boot diskの料金例。購入画面の名称・見積り・提供状況を確認して確定 |
| 地域 | Singaporeを第一候補。availabilityと料金は未確認で、代替地域を勝手に選ばない |
| 稼働上限 | 各VMの作成から最大4時間。受入れの準備が揃う前に課金を開始しない |
| 管理 | 新規試験専用SSH公開鍵。秘密鍵はMacのprivate領域に保存し、チャット／証拠／user-dataへ出さない。管理sourceを制限し、復旧consoleを確認 |
| 追加resource | backup・snapshot・volume・load balancer・managed DB・reserved IP等は追加しない。既存projectやVMへ設定を適用しない |

[公式image一覧](https://docs.digitalocean.com/products/droplets/details/images/)にUbuntu 26.04があることは確認済み。imageが選べることと、Veilの実XDPの合格は別である。

## 費用と本人操作

公式料金例は一台$0.01786／時。二台・各四時間のVM本体は計算上$0.14288。**利用料の承認上限案はUS$1**で、税・通信・追加resourceを含めた作成画面の見積りで確認する。これはプロバイダー側の強制課金上限ではなく、作業側の予算である。作成後の残存resourceを放置しない。[公式料金](https://www.digitalocean.com/pricing/droplets)

公式説明では、クレジットカード登録時に一時的な事前承認が行われる場合がある。PayPalは$5の認証、cryptoは$25の一回のauthorizationと将来利用分へのcredit適用が記載されている。これらを今回のVM本体料金やUS$1の範囲へ勝手に含めない。支払画面に別の金額が出たら本人が確認する。[支払方法](https://docs.digitalocean.com/platform/billing/manage-payment-methods/)

現在のbrowserはDigitalOceanのログイン画面で、account・team・支払方法は未確認。新規account作成、規約同意、本人確認、password／MFA／決済情報の入力は本人が行う。VMはまだ作成せず、認証情報をチャットへ送らない。CLIもこの作業環境では未導入で、tokenを探したり取得したりしていない。

## 課金開始前に揃えるもの

1. 本人のaccount／team、地域・size・image・料金見積り、利用料上限への確認。
2. 公開TLSを確認する専用FQDNと所有者。DNS変更が必要なら対象と戻し方を確認する。未確定のまま公開ブラウザ利用の合格を予定しない。
3. 管理source、BAN用の試験元、独立正常clientの経路。同じXDP入口・保護tupleを通る正常系を用意する。
4. 新規resourceだけを識別するmanifest、証拠の保存先、復旧console、終了時の削除対象・費用停止の確認手順。

具体的な公開IPv4は作成後にしか分からないため、アプリの公開前にその値でfirewall・DNS・Veilのscopeを再確認する。支払方法の登録やaccountへのログインを、VM作成承認として扱わない。

## 通信の設計

- 自宅のF660P、Piの転送・firewall・サービスは変更しない。Macからクラウドへ管理する。
- Cloudflareの通常proxy／Tunnelを今回の利用者別XDP試験経路へ入れない。必要なら専用FQDNのDNS-onlyを別承認で使う。
- 試験元VMのpublic IPv4をBAN対象一件とする。対象は保護対象VMで実際に観測した宛先IPv4／TCP18443。TTL30秒・256件上限・再起動でBANが消える契約を維持。
- 管理SSHとHTTP試験を分け、管理sourceはBAN禁止にする。試験サービスはHTTPS240秒／daemon180秒・Restart=noを維持する。
- 正常な外部clientも同じpublic入口で確認するなら、試験元に加えて正常source一件の許可が必要となる案。**以前の「外部送信元一件だけ許可」からの範囲拡大なので、実値と目的を示して別承認を得る。** 未承認なら追加許可しない。private NICだけの対照結果をpublic入口の正常系へ転記しない。
- IPv6・UDP443・他port等の対象外経路は基礎防御で閉じるか別管理にし、Veilによる保護済みと数えない。実装を拡張して試験を通さない。

## 試験・復旧・削除

最初はアプリportを公開せず、新VMのkernel・vNIC・管理経路、既存XDP／TC、現在のbinary／objectの実行可否を確認する。隔離環境で装着・通信・期限・解除・容量・古いinstance／revision拒否・再起動・停止を確認する。非対応なら権限や安全条件を緩めず、公開前に中止する。

適合後、確定値への承認を得た短いcaseで、公開正常通信、一件BANと期限前解除、別BANの自然失効、通常restartとBAN0を確認する。各caseを先に固定し、失敗を別caseの合格で埋めない。既に合格した同条件を理由なく繰り返さない。

異常時は新設unitだけを停止し、所有XDP解除、対象portの閉鎖、管理・正常通信の復帰を確認する。既存データや未知のプログラムを削除しない。結果不明のBAN・作成要求は再送せず、resourceの実在・状態を読む。

終了時は試験unit停止・非enabled、所有XDP非装着、証拠をMacへ退避してhash照合。削除予定のVM・firewall・鍵登録等をID・名前・所有manifestで列挙し、具体的な削除対象と復旧可否への承認後、新規試験resourceだけ削除する。既存SSH鍵を流用した場合はその登録を削除しない。

DigitalOceanではpower offだけでは課金が続き、VM削除で終了する。削除後にresource一覧と残存する課金対象を確認する。証拠を退避する前にVMを破棄しない。[公式課金条件](https://docs.digitalocean.com/products/droplets/details/pricing/)

四時間上限に達した場合も、削除承認を省略しない。削除できずresourceが残るなら、作業完了・課金停止と報告せず、残存resourceと追加費用の可能性を本人へ明示する。

## 現在の次の一手

本人がDigitalOceanへログイン、またはaccountの新規作成・支払方法の準備を完了する。新規VMの作成はまだ行わない。その後にaccount内の作成画面で上記条件を見積り、具体的な作成対象・影響・復旧方法を提示して承認を受ける。

自宅の追加試験は[受入れ票](DELIVERY_GATE.md)の残り最大一回・三十分という枠で別に扱う。まだ新しい自宅試験を開始しておらず、VM計画の具体化を自宅試験の成功や回数消化に数えていない。
