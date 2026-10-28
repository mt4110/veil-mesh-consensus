# 公開サイト試験導入への最短経路・完了条件・手動BANの採否

2026-10-25 JST。Sol／Highでの確認。**設計とローカル読取りのみ。製品code、既存文書・データ、他service、host・ネットワーク設定には変更を加えていない。**

## ゴールと今回の結論

最終ゴールは、普通のブラウザで利用する小規模公開サイトで、安全に運用・停止・復旧できる無償OSS。
現在の節目は、実装済みのローカル一時BANを、実入口で限定試用して採用価値を判断すること。
今回の完了条件は、Ubuntu待ちで処理できる課題、host準備後の最短手順、技術的合格と運用上の採用を分けた判定条件を確定すること。

**公開の限定試験を準備することは継続可。常用の採用判断は保留。手動BANは有人の臨時対処として条件付きで実用になり得るが、無人の自動防御を満たさない。**
試験で有効な差が確認できなければ、追加daemonを常駐させない。既存防御で足りるならそれを採る。

直近のユーザー決定は「Windows保全、Ubuntuメインのdual boot」「先にLAN／防壁構成を変更する」。
過去の[導入案](2026-10-25-PUBLIC-SITE-PILOT-RUNBOOK.md)の専用OS／VMという候補はこの決定を上書きしない。[OS準備計画](2026-10-25-NEW-PC-UBUNTU-INSTALL-PLAN.md)も同じく、実行前にdual boot用の実disk配置へ具体化する必要がある。既存diskを消す承認はない。
dual bootではWindows起動中にUbuntu上のsiteは停止する。最初は時間を区切った有人試験とし、24時間無人稼働が必要になった時点で運用構成を再判断する。
新しいネットワーク防壁全体の完成を、このOSSの製品要件として追加しない。必要なのは試験hostの入口・sourceの見え方・管理経路を確定できること。

## 現在の証拠と不足

| 対象 | 確認したもの | 現時点の意味 |
| --- | --- | --- |
| 256件・有限TTL・分割掃除 | `local_ban/mod.rs`の定数・処理、既存の回帰・kernel受入れ記録 | ソフトウェアの件数上限。wall-clock時間の絶対保証ではない |
| 起動・停止・復旧 | systemd unit、socket所有規則、既存常設／boot試験記録 | 特定の隔離hostでの証拠。新hostへ合格を転記しない |
| 新binaryのkernel動作 | [受入れ結果](2026-10-25-REBUILT-LOCAL-BAN-KERNEL-ACCEPTANCE.md)、ローカル保存済みresult／最終監査 | 当該実行で27確認が成功。今回の再試験ではない |
| sourceの継続性 | 前回のpilot source manifestの5file、直前の受入れmanifestの3fileが現在のhashと一致 | 今回の調査対象が当該記録から変更されていないこと。全source／実binaryの完全な同一性の新規証明ではない |
| 通信入口 | XDPは1宛先IPv4／TCP port、plain Ethernet・非断片化IPv4を対象 | 物理NIC、driver、実client source、変換後tupleは未確認 |
| 実用性 | status／ban／unban／bypass／inspect-runtime CLIを確認 | 自動検知、BAN一覧、実siteでの効果・運用時間の証拠はない |

既存の主要証拠は`artifacts/rebuilt-kernel-6kmgovmo/`と`artifacts/site-pilot-plan-a0to08k6/`。
remote hostの現在の稼働状態を再取得したものではない。今回はSSH・設定変更・kernel装着を行わない。

## 手動BANだけで実用になるか

手動対応の流れは「兆候を見つける→送信元と影響を調べる→BAN→実通信を確認する」。現daemonが担うのは主に後半。
たとえMap更新が速くても、人間が気付くまでの時間や誤ったIP選定は解決しない。256件は同時BAN数であり、サイト利用者数や攻撃処理能力ではない。

| 用途／条件 | 判定 | 理由 |
| --- | --- | --- |
| 管理者が既に見つけた少数の継続的な迷惑送信元を短時間抑える | 条件付きで有用 | 対象を確認したうえで入口で遮断し、期限・解除・停止で戻せる |
| 所有する試験sourceを遮断して復旧訓練をする | 有用な検証用途 | 攻撃検知精度や実攻撃の防御価値を証明するものではない |
| 寝ている間も攻撃を見つけて守る | 不適合 | detector／通知／判断は実装していない |
| 短いburst、大量にIPを替えるbot、回線飽和 | 主対策として不適合 | 人手の反応、件数上限、hostより手前の帯域という制約がある |
| URL・login user・共有NAT内の特定人だけを制限 | 不適合 | IP／port単位で巻き込む。HTTP側で判断する必要がある |
| CDN／L7 proxy経由で利用者IPを遮断 | 現profileでは不適合 | XDPに見える中継元をBANすると正常利用者全体を止め得る |
| 全IPv4通信を回避不能に遮断したい | 現profileでは不適合 | fragmentはPASS。IPv6／VLAN等も対象外。基礎防御が別途必要 |

**現時点の独自実装を常駐させる必然性は未証明。** 既存手段を比較対象にする。

- [nftablesのset](https://wiki.nftables.org/wiki-nftables/index.php/Sets)は件数上限と期限を設定でき、[要素単位の期限](https://wiki.nftables.org/wiki-nftables/index.php/Element_timeouts)もある。「有限状態と期限付きIP遮断」だけは新規性／優位性の根拠にならない。削除処理がゼロになるという意味ではなく、実装箇所と管理方法が異なる。既存nftablesへの変更は今回行わない。
- [NGINXのrequest rate制限](https://nginx.org/en/docs/http/ngx_http_limit_req_module.html)はHTTP要求に対する既存の選択肢。接続やpacket段階のXDPと防ぐ対象は違う。siteの問題がHTTP要求量なら、まずその基礎防御で足りるかを見る。
- [Fail2Ban](https://github.com/fail2ban/fail2ban)はログを解析して、失敗試行等に基づく一定時間のfirewall遮断を行う既存の選択肢。任意のWeb攻撃を自動で正しく検知する保証ではない。導入先version／filter／proxy trust等は別途確認が必要。

Veilの比較候補の価値は、狭いservice scope、所有者限定操作、応答不明時の再送拒否、起動ごとのinstance、分割掃除、停止による解除等を一組の契約にしている点。
この契約自体と、既存手段より導入・復旧が楽／性能が良いという主張は別。後者は未測定。

## Ubuntu待ちで進められる課題

今回の新設計記録で「調査／設計として済ませたもの」と「実行できないもの」を分ける。

| 優先 | 課題 | 今回の到達点 | 実行側で残るもの |
| --- | --- | --- | --- |
| P0 | 実用性と範囲 | 上の有人一時対処に限定。既存手段との差がなければ不採用 | 運用者が実際に役立つ場面と比較結果を記録 |
| P0 | 受入れの抜け防止 | 下の2gateとcase票を確定 | 実NIC／通信先／管理IP等を埋め、承認して実行 |
| P0 | 新host試験の再現性 | pi固有driverの依存を特定。変更する箇所・維持する条件を明示 | 新host情報確定後にdriverを必要最小限で適合し、隔離実行 |
| P1 | 操作時の人的ミス | instance／revision、TTL、scope、source確認の操作票を確定 | 実運用者が操作し、所要時間・誤操作・復旧を記録 |
| P1 | public向け説明の混在 | READMEのBFT実験説明とlocal BANを区別する必要を確認 | 公開前にpilot入口の説明へ整理。今回は既存文書を変更しない |
| 保留 | 自動検知・遠隔制御・BFT再開 | pilotの前提から除外 | 必要性が実証され、別途合意されるまでは追加しない |

既存`local-ban-rebuilt-check.py`は、FRESH／OBJECT／PREVIOUSの絶対path、UID979、既存namespace、過去のdriver moduleに依存する。
これは当該pi-nodeの証拠を厳しく固定したscriptであり、新host向けinstallerではない。単にpath検査を削除して動かさない。
新host適合時は、明示したbinary／object hash、UID、隔離network、fixture所有確認を設定し直す。27の確認の意味、TTL・期限・満杯時拒否・安全条件は維持する。
未知のnamespace／socket等があれば拒否する。installer兼用、汎用framework化、既存fixtureの流用や削除を追加しない。
この適合はUbuntu待ちの間に仮の値で実装して合格にするより、情報確定後に一度行う方が小さい。

## 最短の経路：環境準備後は二つの判断gate

### Gate 1：試験host／実入口で安全に適用・復旧できる

1. **前提を一枚に確定**：host、一般管理userとconsole、実NIC／driver／kernel、単一宛先IPv4・TCP443、公開FQDN・TLS終端、管理source保護IP、外部canaryと独立正常client、実際のNAT／proxy経路。未確認を推測で埋めない。
2. **同じ隔離受入れを新hostで実施**：新buildまたは選択したbinary／objectのhash、PID実体、正常通信、256／257件、kernel期限、個別解除、通常／SIGKILL restart、旧instance拒否、runtime権限、最終停止を確認する。新hostでのnamespace・account等の変更は先に対象／影響／戻し方を示し承認する。
3. **LANで基礎Webと物理NIC**：最小静的HTTPSと既存基礎防御を確認し、LANの所有clientで装着前→空BAN→一件BAN→解除→停止を確認する。driver未対応・既存XDP競合は停止。権限拡大・既存program置換・勝手なfallbackで通さない。
4. **公開変更を具体的に承認**：専用FQDN、router／FW／DNS／TLS／unitの差分、対象外通信、試験時間帯、観測終了時刻、console担当とrollbackを提示する。公開経路から実sourceが見えなければ利用者IP向けXDP pilotは中止する。
5. **公開canary一件だけ**：正常clientは継続観測。基礎防御だけのbaseline→空BAN装着→canary BANと期限失効→新BANと個別解除→新BANとbypass→bypass解除後に別BANと通常restart→停止を区別して検証する。終了時は停止・非enabled、XDP非装着と通信回復を確認する。

上記はすべての設定変更を現在承認するものではない。新PCのOS準備はWindows保全の別計画を守る。
物理NICを確認せず公開試験に飛ばない一方、公開前に大規模性能試験・分散合意・自動検知を追加しない。

### Gate 2：継続導入する価値がある

Gate 1合格は「限定pilotを技術的に実施できた」であり、常用の採用決定ではない。

- まず基礎防御だけで解決しない具体的な対処場面を一つ示す。なければ人工的な課題を作って製品価値の証明にしない。
- 同じ所有source・同じ試験条件で、Veilあり／なしを比較する。必要なら既存firewallの期限付き遮断も候補とするが、別途承認した隔離環境で行う。網羅的な防御製品benchmarkへ広げない。
- 正常HTTPSの成功／遅延、host CPU・RSS／BPF memoryの範囲、Web側へ届いた処理量、検知から確認までの人手時間、操作回数、誤操作・復旧を記録する。kernel drop counterは全体値であり、特定IPの効果や全通信の成功を単独では証明しない。
- 同時実行量・試験入力・測定時間・許容劣化・必要な効果・運用負担の許容値は、site baselineを見て**比較試験前に**運用者と確定する。未測定のRPS／ms／改善率や恣意的な固定値で合格を作らない。
- 正常利用者の巻込み、管理喪失、未知XDP、効果確認不能、既存手段との差なし、手動対応の負担超過なら不採用／保留。自動検知追加を合格するまでの無限の後続開発にしない。
- controlled trafficの結果は、その条件の効果として報告する。試験送信元の遮断成功だけで実攻撃対処や24時間の無人防御を達成したとしない。

Gate 2は必要な場面と受入基準が確定してから実施する。観測期間・公開日付は未合意であり、本書では勝手に日数を設定しない。

## 公開pilotのcase票

各caseでcommandのreturn code、操作前後のinstance／revision／deadline、kernel counter差、canaryと正常clientの独立したHTTPS結果を最小限記録する。
IP等はprivateに保持し、payload、cookie、認証情報、秘密鍵を収集しない。失敗・未実施もそのまま残す。

| Case | 操作 | 合格条件 |
| --- | --- | --- |
| P01 | 基礎防御のみ→空BAN装着 | 正しい証明書・期待bodyのHTTPS成功、管理経路と対象外通信を維持、装着先／binary／object照合 |
| P02 | 所有canaryへ有限BAN | 対象IPv4/TCP通信を遮断しcounterも増加。独立正常client・管理・対象外portは継続。IPは共有egressを巻き込まない |
| P03 | 自然失効 | 元のkernel deadline以降に通信復帰。再BAN／期限延長／userspace全件削除を成功理由にしない |
| P04 | 新しい有効BANからunban | 操作直前も遮断・未失効を確認。unban後の通信復帰。期限失効を解除成功へ取り違えない |
| P05 | 新しい有効BANからbypass | 現instanceと**control_revision**で緊急socketを操作し通信復帰。BAN用revisionと混同しない |
| P06 | bypass off確認後、別BANから通常restart | restart前の遮断を確認。restart後は新instance／BAN0／通信復帰。旧instance要求は拒否 |
| P07 | 所有unitの停止 | XDP非装着・正常HTTPS・管理通信を確認。停止要求が返っただけで復旧済みとしない |
| P08 | 観測終了 | 所有unit停止・非enabled。今回変更の最終状態と保全対象の照合。継続・enableは別判断 |

初回の期限入力案は前runbook同様30秒。まだ未承認なので、実行時に確認する。command待ち期限を延ばして通したり、結果不明のmutationを自動再送したりしない。
bypass offは有効BANを再び効かせるので、canary以外にBANがないこととその影響を確認してから行う。
scope外へのPASSを防御成功に数えない。fragment／IPv6等の基礎防御・露出を別に確認し、それらもVeilで遮断できることが必要なら現profileは不採用。
256件満杯、SIGSTOP／SIGKILL、所有記録破損は隔離環境で確認し、公開siteには障害注入しない。

## 操作票：人間が判断する部分

BAN前に「何が問題か」「そのsource IPだと判断した根拠」「誰を巻き込むか」「対象service」「有限TTL」「失効前に解除／停止できる経路」を記録する。
statusで現instanceとrevisionを読み、構文を確認して一度だけbanする。APPLIEDはMap更新成功であり、実通信の遮断まで確認する。
応答不明なら同じ操作を盲目的に再送せずstatusと通信を確認する。revisionを取り直して自動再BANしない。
現CLIは各IPのstatusで、active一覧やevent本文の表示commandはない。複数の実対処を人が追跡できることは未検証。case票で一件ずつ扱うpilotには進めるが、多数IPを手動管理できるとは主張しない。
この作業が負担になる場合は、まず安全な操作支援で解決できるかを別途検討する。検知・BANの自動化を無断で追加しない。

## 今回の完了と未完了

今回で、実装・記録の突合、既存手段との比較による手動BANの条件付き評価、二つのgate、公開case、操作・停止判断を設計として確定した。
実機待ちの未解決値はhost／disk、NIC、公開経路・FQDN、管理source、canary／正常client、基礎防御、測定入力と受入予算。
これらが埋まるまで配備はHOLD。新機能の追加で待ち時間を埋めず、必要な情報がそろったらGate 1を実行する。

既存codeを再build・実行していないため、新しい回帰成功やhostの現在状態は報告しない。本書と新規ローカル証拠だけを作成し、既存の未コミット変更・fixture・データ・ネットワークは保全する。
