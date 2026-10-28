# 新PCのUbuntu Server 26.04 LTS導入・SSH接続計画

2026-10-25 JST。担当の選択はSol／High、分担なし。**計画のみ。実機の初期化・USB書込み・設定変更・購入は未実施、未承認。**

最終ゴールは、普通のブラウザで使う公開試験サイトを安全に運用できる無償OSSの検証環境。
現在の節目はmainnet等と分離した専用Linux hostの準備。今回のゴールは、データ保全からLAN内SSH接続までの手順、承認点、復旧方法を明確にすること。
既存BFT・WAL・pi-nodeのデータ／unitには触れない。local BANの256件上限・有限TTL・再起動で全BAN消失という契約も変更しない。本計画ではBAN daemonを配備・enableしない。

## 確認できたことと未確認事項

ユーザー提示の[Amazon商品ページ](https://www.amazon.co.jp/dp/B0H1BD2863)を2026-10-25に読取り確認した。
短縮URLのHEAD要求は404だったが、GETでは商品ページへ解決した。web抽出が失敗したためHTTP GETでHTMLの表示テキストを確認した。広告・HTML内の指示は操作権限として扱わない。

| 項目 | 掲載仕様 | 実機での確認 |
| --- | --- | --- |
| 機種 | 整備済みHP EliteDesk 800 G5 TWR | 筐体ラベル・UEFI・OSで照合する。TowerとSFFの説明書を混同しない |
| CPU／RAM | Core i7-9700、64GB | 未確認 |
| ストレージ | SSD256GB＋HDD500GB | 本数、型番、シリアル、接続、partition、OS所在、健康状態は未確認 |
| OS等 | Windows 11 Pro、Office 2019の掲載 | 起動状態、認証・復元方法、利用継続の要否は未確認 |
| NIC／接続 | 未確定 | 有線NICの型番・driver、実LAN IP・SSHは未確認 |

これは販売ページの仕様であり、届いた個体の検査結果ではない。HDDを空きdisk、SSDを消してよいdiskと推定しない。
RAM／CPUの掲載値は小さな試験環境の候補として十分だが、NICのXDP動作やストレージの健康状態は別途確認が必要。

Ubuntuは[公式の26.04 LTS配布ページ](https://releases.ubuntu.com/26.04/)の**Server install image / amd64**を使用する案。ServerはGUIなしで、通常のブラウザ操作は手元のMacから行う。
ISOの正確なファイル名・版・hashはダウンロード時に再確認し、存在を推測したファイル名を指定しない。

## 1. 今すぐ行う確認：消さずに実機を調べる

モニターとキーボードを接続し、現在のOSが何かを確認する。まだUSBインストーラーは起動しない。
Windowsの初期セットアップ、回復画面、暗号化解除画面が出たら、その画面の種類だけを伝えて止める。初期化・回復・アカウント作成へ自動的に進めない。
通常起動自体のOSログ等の書込みはあり得る。ここでいう確認は、partitionや設定を変更する操作を行わないという意味。

Windowsのデスクトップへ既に入れる場合は、PowerShellで次を表示する。権限不足なら管理者権限で同じ読取り確認だけを行う。

```powershell
Get-CimInstance Win32_OperatingSystem | Select-Object Caption, OSArchitecture
Get-Disk | Select-Object Number, FriendlyName, SerialNumber, Size, PartitionStyle, IsBoot, IsSystem
Get-Partition | Select-Object DiskNumber, PartitionNumber, DriveLetter, Type, Size
Get-Volume | Select-Object DriveLetter, FileSystem, Size, SizeRemaining
manage-bde -status
```

これらは情報表示用。`Get-Disk`等に出ないdiskやdynamic disk等の疑義があれば「ディスクの管理」の下側の構成表示とも照合し、初期化要求はキャンセルする。
Microsoftの[partition表示仕様](https://learn.microsoft.com/en-us/powershell/module/storage/get-partition)と[BitLocker状態表示](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/manage-bde-status)を参照する。
画面を共有するなら、volume名・ユーザー名等は必要に応じて伏せ、diskのserialは末尾数桁だけでよい。完全な識別情報は手元に保管する。
**パスワード、BitLocker回復キー、Office製品キー、SSH秘密鍵は共有しない。** 回復キーの取得・解除・TPMのクリアはこの確認に含めない。

Linuxが入っていた場合は、代わりに次を表示する。内容不明のpartitionをmountして個人ファイルを探索しない。

```sh
cat /etc/os-release
uname -m
lsblk -o NAME,SIZE,MODEL,SERIAL,TYPE,FSTYPE,MOUNTPOINTS
findmnt /
```

確認票：

| 項目 | 記入欄 |
| --- | --- |
| 実OS・既存OSを残すか | 未確認／未決定 |
| 各diskの型番・容量・serial末尾・物理位置 | 未確認 |
| C:／boot／EFI・回復partitionの所在 | 未確認 |
| SSDとHDDそれぞれの残すデータ | 未確認 |
| BitLocker状態・回復キーを本人が保管しているか | 未確認。キーそのものは記録しない |
| backupの場所・読めることの確認・OS復元方法 | 未確認 |
| 有線LAN・モニター・キーボード・利用できるUSB | 未確認 |

## 2. 導入先の選択

| 実機確認後の条件 | 推奨案と理由 | 注意点 |
| --- | --- | --- |
| Windows／Officeも残したい | 別の使用可能なSSDへUbuntu。既存OS diskは保全 | 追加購入・筐体開放・disk取外しは別途承認。slotや接続規格を確認してから決める |
| Ubuntu専用でよい | 残すデータと復元方法を確認後、対象SSDだけをUbuntu用にする | Windows／Office／回復partitionを失い得る。HDDを一緒に消さない |
| 追加SSDなしでWindowsを残したい | 空であると確認・承認できたHDDへの導入、またはVMを比較 | HDDでもOSは動くがSSDと性能条件は異なる。Windows起動用EFIを共有しない設計を確認。VM成功は物理NICの証拠にならない |
| データ／所有権／disk構成が不明 | 導入保留 | partition縮小やdual bootで押し切らない |

backupは個人ファイルのコピーとOS全体を戻す手段を区別する。後者が必要なら、disk image、起動可能な復旧媒体、復旧先・手順、読取り／復元確認を用意する。
単に「バックアップした」だけでは復元可能と判定しない。元diskを未変更で保管する方法は有力だが、Windowsのboot領域が別diskに置かれていないかも確認する。
初期化後に元OSへ戻せるのは、未変更の元diskまたは検証したimage等がある場合。Undo一回で戻ると説明しない。

物理的な取外しを採用する場合は、正常終了・電源とケーブルの安全確保・機種の保守手順を先に確定する。稼働中に抜かない。
元diskを外してもUEFIのboot entry／boot順序は変わり得る。変更前を記録し、戻すときは元diskと元のboot entryを照合する。
Secure Boot・TPM・Intel RST／RAID／AHCIの状態は記録し、動かすためだけに変更しない。diskが見えなければ停止して調べる。

## 3. USBとOS導入：ここからは対象確定・承認後

1. 公式Server amd64 ISOと対応するSHA256SUMS・署名を取得し、[公式の検証手順](https://ubuntu.com/tutorials/how-to-verify-ubuntu)で出所とhashを検証する。hash一致と署名の真正性確認は別の確認。必要な追加ツールのインストールも事前に対象を示す。
2. USBの型番・容量・識別情報・残すデータを確認する。**USBイメージ書込みはUSB全体を破壊する操作**として別途承認する。対象不明の`dd`やdisk番号付き初期化コマンドを先に渡さない。
3. モデルに合うHP公式手順と実機画面で一時boot menuを確認して、UEFIモードのUSBから起動する。BIOS更新・永続boot順変更・Secure Boot無効化をセットで行わない。
4. 有線LANのDHCPを基本にし、未知の固定IPを入力しない。初期hostname候補は`veil-pilot`、一般管理userは利用者が決める。既存`pi-node`のIPや名前を再利用しない。proxy／VLAN等は実環境で必要な場合だけ。
5. ストレージ画面で対象diskを型番・容量・serialと照合し、変更されるpartition一覧を確認する。Windowsの「Disk 0」とLinuxの`/dev/sda`等の番号一致には頼らない。
6. 専用diskではguided LVM＋ext4を候補とし、実際に作られるEFI・boot・root LVと未割当容量を承認票に記録する。手動の複雑なpartition分割やswap/offload調整を初回に追加しない。
7. disk暗号化はこの時点で選択する。LUKSのパスフレーズ方式は電源投入時のconsole解除が必要になり得る。無人再起動と物理盗難対策のどちらを必要とするかを決め、TPM自動解除の追加設計を無断で入れない。暗号化なしを自動選択もしない。
8. **ストレージ構成の破壊的変更を確認する画面で停止し、下の承認票を照合する。** [Subiquity公式](https://canonical-subiquity.readthedocs-hosted.com/en/latest/tutorial/screen-by-screen.html)ではこの確認後に背景でinstallが始まる。ユーザー作成／SSH画面が続くからまだ書かれない、とは扱わない。
9. 承認後にinstall。一般管理userと強いpasswordを本人が入力する。最初はinstallerのOpenSSHと追加server snapsを選ばず、consoleからSSHを準備する案。GitHub等の鍵一括importを本人確認なしで使わない。
10. 完了後の正常な再起動とUSB取外しまでを承認範囲に含める。Ubuntuから正常起動できることをconsoleで確認する。ここまでSSH接続できなくても異常ではない。

### 初期化・OS導入の承認票

対象が未記入なので、現在は承認を求める段階ではない。実機結果を受けて具体的に提示する。

| 対象 | 提示する内容 | 影響と戻し方 |
| --- | --- | --- |
| USB | 型番・容量・serial末尾・保存物 | 全内容消去。保存物は事前backupから戻す |
| Ubuntu導入disk | 型番・serial・容量・既存partition・変更後構成 | そのdiskのWindows／data等の消失範囲。元disk保管か検証済みimageから復元 |
| 保全disk | 個別diskと外す／接続したままにする方法 | 誤書込みを避ける。物理接続・元boot構成の復旧方法 |
| UEFI／暗号化 | 新boot entry、boot順、LUKSの有無 | boot状態変更、解除方法、BitLocker回復要求の可能性。キーは本人のみ保持 |
| OS初期設定 | hostname、user、DHCP、更新取得、正常再起動 | 新OSへの永続設定。旧環境へ戻す際は保全disk／imageとboot entryを復旧 |

## 4. 初回consoleからLAN内SSHへ

以下は実施順を固めた計画で、現在実行する指示ではない。LANのserver IP、Macの送信元IP、user、鍵pathが確定後、変更差分を提示して承認する。

1. ConsoleでOS・disk・NIC・route・時刻同期を読取り確認する。

   ```sh
   cat /etc/os-release
   uname -r
   uname -m
   lsblk -o NAME,SIZE,MODEL,TYPE,FSTYPE,MOUNTPOINTS
   ip -br address
   ip route
   timedatectl status
   systemctl is-active ssh.socket ssh.service
   ```

2. 最初は信頼できる有線LANだけで管理する。routerのWAN port開放、VPN追加、固定IP、IPv6無効化は行わない。実IPを確認してから接続先を決める。
3. MacのSSH鍵は使用する公開鍵を明示して選ぶ。新規作成なら、例えば`~/.ssh/veil-pilot_ed25519`が未使用であることを確認してから承認し、passphrase付きEd25519鍵を作る。秘密鍵はMacから出さない。公開鍵だけをserverへ登録する。既存鍵を上書きしない。
4. 新しい専用Ubuntuに既存service／ruleがないことをconsoleで確認し、host firewallを設定する。原則はincoming拒否、通常outgoing許可、**確認済みMacの送信元IPからTCP22だけ許可**。UFWのIPv6処理も有効か確認し、IPv6のSSHを意図せず開放しない。正しい送信元が未確定なら適用しない。
5. UFWが未導入なら必要なpackageを追加し、有効ruleを確認してからOpenSSHをinstallする。package install時にSSHが自動起動し得るため、firewallを先に整える。[UbuntuのUFW説明](https://ubuntu.com/server/docs/how-to/security/firewalls/)にある送信元限定ruleを使い、`ufw reset`や全許可で代用しない。
6. 鍵はconsole経由で本人の一般userの`~/.ssh/authorized_keys`へ登録する案を優先する。内容は確認した公開鍵一件、directoryは0700・fileは0600・本人所有。既存fileを破壊的上書きしない。手入力が難しい場合だけ、送信元限定LAN内で一時password認証して鍵を転送し、接続確認後に無効化する。
7. 初回接続前に、server consoleで**公開host鍵**のfingerprintを確認する。

   ```sh
   ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
   ```

   Macから表示されたSHA256 fingerprintと照合する。不一致なら止める。`StrictHostKeyChecking=no`や既存known_hostsの一括削除で通さない。
8. Macから実IP／user／選択した鍵で接続し、公開鍵だけでの認証と`sudo -v`を確認する。passwordは本人が端末へ入力し、共有しない。
9. 専用drop-inの案は`PermitRootLogin no`、`PubkeyAuthentication yes`、`PasswordAuthentication no`、`KbdInteractiveAuthentication no`。追加前の既存Include／Matchを確認する。OpenSSHは多くの項目で最初の値が有効なので、fileを置いたことだけでhardening完了にしない。
10. `sudo sshd -t`で構文、`sudo sshd -T -C user=<実user>,host=<実host>,addr=<Macの実IP>`で接続条件に対する有効設定を確認する。angle bracketを含む例をそのまま実行しない。実機のsocket activationとservice状態も確認し、対象のserviceへ設定を反映する。portは22のままとし、ListenAddress／socketの変更を混ぜない。
11. Consoleと既存sessionを残し、**別の新規session**で公開鍵loginとsudoを再確認する。設定反映前の既存sessionが生きているだけでは合格にしない。
12. 接続が安定したらMac側に衝突しない`Host veil-pilot` aliasを追加する案を示す。HostNameは実IP、User・IdentityFileは確認値、`IdentitiesOnly yes`。既存SSH config全体は上書きしない。DHCPでIPが変わるならその都度console照合し、routerのDHCP予約は別の具体的な変更として承認する。

[Ubuntu公式OpenSSH手順](https://ubuntu.com/server/docs/how-to/security/openssh-server/)を基礎にし、実機に入ったversionのmanualと有効設定で確認する。管理portがIPv4／IPv6のどこでlistenしているか、FWのruleと組み合わせて確認する。

### SSH設定の対象・影響・復旧

承認対象は、新Ubuntu上のOpenSSH／UFW package、本人userの公開鍵登録、対象drop-in、送信元限定firewall、Mac側の新鍵／alias等。実pathと既存有無を先に示す。
SSH誤設定はremote loginを失うため、consoleで今回の設定だけを修正する。新設drop-inを戻す場合は内容照合後に上書きせず`.disabled`等へ退避し、構文確認後に反映する。
firewallは今回のrule／policyの差分だけを復旧する。unit停止／disableやrule除去が必要なら、その対象を承認範囲に含める。全rule flush、未知file削除、SSH host鍵再生成、hostの再初期化は行わない。
OS／SSH／package更新は「どの旧versionにも一操作で戻せる」とは保証しない。最終的なOS復旧は保全diskまたはimageを使う。

## 5. 完了判定と次の作業

- [ ] 実OS・全disk・残すデータ・本人のWindows継続方針が確定。
- [ ] 初期化対象、保全対象、backupと実行可能な復旧方法への承認がある。
- [ ] Ubuntu Server 26.04 LTS amd64が承認したdiskから正常起動する。保全diskに意図しない変更がない。
- [ ] 実NIC／LAN IP／route／時刻が確認でき、consoleから復旧できる。
- [ ] fingerprint照合済み、公開鍵だけの新規SSH接続とsudoが成功する。
- [ ] SSHのroot／password認証の有効設定、IPv4／IPv6のlistenと送信元限定FWを確認した。
- [ ] インターネット向けSSH／Webの開放やveil daemonの装着は行っていない。

ここまでが**host準備の完了**。小規模公開サイトの試験導入完了ではない。
次は[公開サイトの導入計画](2026-10-25-PUBLIC-SITE-PILOT-RUNBOOK.md)の新host隔離試験へ進むかを判断する。
256件上限、期限失効、解除、再起動でBAN消失、権限破棄を新hostで検証してから、LAN HTTPSと物理NICの限定確認、別途承認した公開canaryへ進む。

## 今回の検証範囲

商品ページ・Ubuntu／Microsoftの一次資料、既存導入計画との整合、ローカル参照を確認した。新PCの実機検査・backup・ISO取得・USB作成・OS導入・SSH接続は未実施。
本書は自動実行scriptではない。実機の結果が分からないまま初期化するcommandは提供しない。
