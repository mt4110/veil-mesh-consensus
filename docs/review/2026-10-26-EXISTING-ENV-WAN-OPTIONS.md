# 既存環境だけで行うWAN試験の選択と停止条件

2026-10-26 JST。設計評価のみ。最終ゴールは普通のブラウザが使う公開サイトでの安全な試用。今回の完了条件は、新しい外部サーバーを借りず、Piの設定・サービスを変えず、許可する送信元・portを広げない候補を比較し、次に行う最小の確認を決めること。

**後続実測:** 本人承認後、Macだけの外向き測定で、固定した送信元endpointと保持接続による二宛先・約50秒・12応答の同一IPv4を確認した。[結果と復元](2026-10-26-MOBILE-FIXED-SOURCE-PORT-MEASUREMENT.md)を参照。以下の未実施表記は設計時点の記録。hpへのWAN到達・BANは引き続き未確認。

## 判断

**次の第一候補は、Mac側だけで「TCP送信元ポートを固定し、外向き接続を保持する」方法の適合確認。最初の段階ではhp／Pi／ルーターを変更しない。** 固定IP契約や新規サーバーは、この候補の必須条件ではない。

ただし、キャリア等の外側の変換・中継がこの方法で同じIPを維持するかは未検証。成功するまでルーターの許可sourceを変更し続けたり、許可をany／CIDRへ広げたりしない。WANでのIPv4 XDP一件BANを実施できると確定した設計ではない。

共用ルーターへの保存を含めたPiの完全な無瞬断を、未検証のまま保証することもできない。その保証が必須なら、ルーター変更を要する直接IPv4のWAN段階は実施しない。ルーターを変えない外向き調査や、後述のTunnelによるサイト閲覧確認までを範囲とする。

## 前回の観測から断定できないこと

[前回の記録](2026-10-26-WAN-SOURCE-PREFLIGHT.md)では、新しいcurl接続のたびに外向きIPv4を照会した。短時間に異なるIPを観測したことは事実だが、次の要因は分離できていない。

- 時間経過・無線再接続に伴うIP変化。
- 新しいローカル送信元TCPポートに対する、別のNAT変換。
- 照会先IPが異なることによる、宛先依存の変換。
- 外側の中継等による差。どの装置・契約方式で変化したかは未確認。

照会先で見たIPと、hp向けの接続の実sourceは同一と証明できていない。したがって「動的IPだから新規外部サーバーが必須」「IP変動だけがTCP失敗原因」とは言えない。

## 推奨候補：クライアントの送信元を保つ

TCP接続の識別には送信元IP・portと宛先IP・portを使う。ローカルIP／portを同じに保ち、宛先だけを変えて通信することはOS上で可能。
[RFC 5382の4.1](https://www.rfc-editor.org/rfc/rfc5382.html#section-4.1)は、同じ内部IP／TCP portについて宛先によらない変換をEndpoint-Independent Mappingと定義し、TCP NATの要件としている。[RFC 6888のREQ-2](https://www.rfc-editor.org/rfc/rfc6888.html#section-3)にもCGNのIP poolingに関する要件がある。ただし、これらは現在の回線の適合証明ではない。

```text
Mac：携帯側の同じローカルIPv4＋同じTCP送信元port P
  ├─ 既存のHTTPS照会先Aへ接続して保持
  └─ 別の既存HTTPS照会先Bへ接続して実sourceを比較

二宛先で同じsourceを維持できた場合だけ、別承認の段階へ
  └─ 同じIPv4＋port Pから、hpの試験TCP18443へ接続
       └─ hpで実sourceが承認済みIPv4と一致することを検証
```

### 段階1：Macだけでの適合確認

将来実施する場合の案。今回、携帯IPv4の設定変更や実NAT試験はしていない。

1. 必要なら承認済みの一時Wi-Fi DHCPを使い、有線管理と区別する。
2. clientが所有する空き送信元portを一つ確保し、携帯interfaceとローカルIPv4を明示する。他processのportを奪わない。
3. 既存のHTTPS IP照会サービスにTLS検証付きで接続し、接続を保持する。新しいサーバーの配備や公開はしない。Cookie・認証情報を送らない。
4. 同じ送信元IP／portで、別の実宛先IPへの接続でもsourceが一致するか確認する。例は同じ既存サービス名の異なるA recordで、TLSの検証対象hostnameは維持する。実際に異なる宛先があることをその時点で確認する。
5. 接続の保持は最大180秒、照会は最大12回、同時socketは最大3個、応答bodyは各4KiBまでとする案。切断時に自動再接続して別sourceを探さない。
6. IP・携帯interface・ローカルaddressが変わったら中止し、自分のsocketを閉じ、一時設定を戻す。

ここで確認できるのは「調べた二宛先・この時間帯でsourceを維持できた」こと。すべての宛先に対するNATの種類や、hp向けのsourceまで確定したとは報告しない。

### 段階2：同じ許可範囲でのWAN候補

段階1が成立してから別途、確定値・影響・復旧を確認する。PiへのVPN・ルート追加、既存Pi転送の流用はしない。

- F660Pの試験ルールは承認source一つの/32、hpのTCP18443一つだけ。旧sourceとの和集合を作らない。全体firewallを下げない。
- hpのサービス単体制限も、LAN正常確認元とその/32だけ。管理SSHやhost全体の既定policyを変更しない。
- 照会先との接続と同じclient portを使ってhpへ接続し、TLS・nonce・hpが見たpeerを照合する。source不一致・到達失敗ではXDPへ進まない。
- 外向き接続の保持は携帯IPの予約ではない。保持中に変換が維持されなかった場合も中止し、sourceを自動追従させない。
- 送信元変換が安定しても、F660Pや回線の受信filterを通過する証明にはならない。NATの変換方式と受信許可は別条件として判定する。
- sourceの/32一つは、本人一台の認証ではない。携帯の共有NATでは他利用者も同じIPを使い得る。試験endpointには秘密・管理操作・既存の利用者dataを置かず、管理はLANのUDS経由に保つ。
- BANは既存製品・既存のexact宛先scope・有限TTL30秒で一件。daemon180秒／HTTPS240秒、Restart=no、製品の管理I/O期限や256件上限は変えない。
- 同じTCP portで新しい接続を作る場合はTIME_WAITやNATの接続状態を検証する。OS全体のTCP設定を変更して回避しない。検証用client自身の接続だけを打ち切って再利用する候補はあるが、TLS応答全体を確認後に限定し、他の接続には触れない。
- 既存接続上の遮断だけを確認した場合は、その範囲で報告する。新規接続、解除後の再接続、再起動後の到達まで自動的に合格へ含めない。

実NATでの反復失敗を前提にしない。段階1の適合確認1回、成立した場合のWAN予備確認1回を判断単位とし、不成立なら保留する。期限・許可範囲を後から変えて通さない。

## 今回行った小さな確認

| 確認 | 結果 | 証明しないこと |
| --- | --- | --- |
| Macのloopback上で、同じclient portから二つの別宛先へ同時接続 | 成功。両接続の実port一致を照合し、全socketを閉じた | Wi-Fi経路・キャリアNAT・WAN到達 |
| 一方の所有接続だけを打ち切り、同じ宛先へ同じportで再接続。他方は保持 | 成功。他方の接続も通信継続。全体TCP設定は変更なし | キャリア側の接続状態やTIME_WAIT処理、通常ブラウザの動作 |
| 携帯側IPv6を指定した既存HTTPS照会 | HTTP200。clientのlocal IPv6と照会先が見たIPv6が一致 | hpへのIPv6到達、IPv6 addressの長期安定、IPv4 BAN |

loopbackの二つの受信socketは127.0.0.1にだけbindし、確認後すべて閉じた。受信portの外部公開はない。今回、Piへのコマンド実行・設定変更、hp設定変更、ルーター操作、製品変更、IPv4 WAN試験再開はしていない。

## 他の案との比較

| 案 | 借用サーバー | Pi／共用ルーターへの変更 | 現行IPv4 XDPのWAN検証 | 判断 |
| --- | --- | --- | --- | --- |
| 固定したclient port＋接続保持 | 不要 | 最初の外向き調査は両方なし。実WAN段階のみ試験転送の承認が必要 | NATと実peerの条件が成立すれば候補 | 次の適合確認として推奨 |
| 携帯IPv6→hp IPv6、source /128・専用port | 不要 | Piなし。共用routerのIPv6受信rule検討が必要 | できない | HTTPSのWAN疎通だけを別目的で試す候補 |
| Cloudflare Tunnel＋試験host名限定Access | 不要 | hpに専用connector、Pi／router変更なしの案 | 元の利用者IPのBANは検証できない | サイトの外部閲覧・TLS・停止を先に試すなら候補 |
| Piの既存VPN／公開portを試験に流用 | 不要 | Piの経路・負荷・既存機能へ依存 | 経路次第で別物になる | 今回は不採用 |
| 送信元CIDR／anyへ拡張、mTLSだけで代替 | 不要 | 受信許可集合が増える | 機能確認は可能でも条件違反 | 不採用 |
| NAT loopbackやLAN内から公開IPへ接続 | 不要 | 構成次第 | 本来の外部経路を証明しない | WANの代用にはしない |

### IPv6案の限界

現在のC filterはEtherType 0x0800（IPv4）以外をPASSする。IPv6のsource /128一つに限定しても、現行製品のBAN試験にはならない。新しいaddress familyの受信許可は従来のIPv4 /32と同じruleではないため、許可を広げないという条件を勝手に満たしたことにしない。家族全体のIPv6設定を変えたり、IPv6対応を今回のために製品へ追加したりしない。

### Tunnel案の限界

[Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/)はhpからのoutbound接続で構成できるので、F660Pの新規転送を必要としない。新規試験host名だけへ[Accessのdefault-denyと本人だけのAllow](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/self-hosted-public-app/)を先に設定する案になる。既存siteやアカウント全体のpolicyは変更しない。料金・アカウント条件は作成前に確認し、今回契約・配備はしていない。

これはIP一件制限から本人認証へ方式を変える案であり、同じ許可ruleのままとは説明しない。hpのNICでは利用者の元のL3 sourceが見えないため、Tunnelの成功と既存LANのBAN成功を合算して「WANで一件BAN成功」としない。

## 今回の完了と次の判断

設計比較とローカルOSの前提確認は完了。キャリアNAT適合・実WAN到達・WAN BANは未実施。推奨する次の作業は、**ルーターもPiも触らない、固定送信元portを使った外向きNAT適合確認**。その結果が成立してから、従来と同じ許可集合のWAN試験へ進めるか判断する。

証拠は[二宛先への同時接続](../../artifacts/wan-existing-env-design-iwbazoti/loopback-source-port.json)、[所有接続の再接続と他方の継続](../../artifacts/wan-existing-env-design-iwbazoti/loopback-reconnect.json)、[今回のIPv6外向き観測の要約](../../artifacts/wan-existing-env-design-iwbazoti/ipv6-observation.json)。loopbackの成功をNATの成功へ転記しない。現行の[公開前チェックリスト](../local-ban/PILOT_CHECKLIST.md)と既存の失敗記録を継続使用する。
