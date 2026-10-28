# Macの固定送信元TCPポート・保持接続による携帯側外向きIPv4の測定

2026-10-26 JST。最終ゴールは公開サイトでの安全な試用。今回は[既存環境の選択肢評価](2026-10-26-EXISTING-ENV-WAN-OPTIONS.md)の段階1だけを実施し、Macからの外向き接続でsourceを維持できるか確認した。

**今回の二宛先・約50秒の測定では成立。同じローカルIPv4／TCP送信元portを使い、同じ2本のTLS接続を保持した12応答すべてで、同一の外向きIPv4を観測した。Wi-Fiは元へ復元済み。Pi・hp・ルーターへの操作、WAN入口の開放は行っていない。**

これは当該条件での観測であり、キャリアNAT全体の動作保証、hpへのWAN到達、WANでのBAN成功ではない。

## 方法と固定条件

| 項目 | 実施内容 |
| --- | --- |
| Macの接続 | 有線en0を管理経路に維持。Wi-Fi en1だけ承認範囲で一時DHCPへ変更 |
| ローカル送信元 | 携帯側IPv4の172.20.10.2、OSが割り当てた空きTCP port 56805。2接続で同じ組を使用 |
| 宛先 | `www.cloudflare.com`のその時点の異なるA record、104.16.123.96:443と104.16.124.96:443 |
| interface固定 | SDKに定義されたIP_BOUND_IFを使用。TLS接続後もbound interface、local／remote endpointを照合 |
| TLS | 通常のCA検証とhostname検証を有効のまま実施。両接続TLS1.3、HTTP/1.1。証明書検証の無効化なし |
| 照会 | 同じ2本のTCP／TLS接続で/cdn-cgi/traceを約10秒間隔、6組、計12回。切断時の自動再接続なし |
| 上限 | 全体180秒、12照会、同時socket3個以内、応答body各4KiB。実際のTCP接続は2本 |
| 測定時間 | 08:44–08:45 JST。client処理50.177秒、最初から最後の応答観測間は49.478秒 |
| 保存内容 | local／remote endpoint、FD、TLS version、HTTP status、反映されたIPv4と時刻。Cookie・認証情報は送信せず、照会body全文は保存しない |

既存HTTPSサービスへの通常の照会だけで、新規サーバー、受信listener、公開DNS、トンネル、ルーター転送、PiのVPN／公開portは使用しなかった。TLS通信用のHTTP request／responseは測定に必要な範囲で処理するが、traceの全bodyやCookie等を証拠として保存していない。

## 結果

- local endpointは全12応答で同一。
- 宛先は実際に異なるIPv4二つ。接続先hostnameは同じであり、二つの独立した事業者や全宛先の試験ではない。
- 各接続は6応答を返し、FD・local／remote endpointを維持。途中の再接続なし。
- HTTPは全12回200。反映された外向きIPv4は全12回同一。値はprivate証拠に限定。
- helper／wrapperは終了0。作成した全socketを閉じた。
- 原結果とは別に、request数、宛先数、local endpoint、外向きIP、各FDの継続、時間上限、終了・復元を再集計して照合した。

## 復元と操作範囲

Wi-Fiの変更前情報をprivateに保存し、既知の元状態に一致することを確認してから自動DHCPへ切り替えた。wrapperのfinallyで元の手入力アドレス付きDHCP・192.168.1.9へ戻した。

終了後に別コマンドでgetinfoを読み、Wi-FiとEthernetの表示設定が変更前と一致することを確認。hp宛て・Pi宛て・defaultの経路もinterface／gateway等が一致し、管理宛てはen0のまま。これはMacの経路の確認であり、今回は両hostへのSSH疎通やアプリ機能の検査を追加していない。

Pi・hp・ルーターにコマンドは送っていない。既存防御、許可範囲、製品、OS全体のTCP設定、IPv6設定は変更していない。WAN入口を開く操作、XDP装着、BAN、再起動、外部公開は未実施。

## 解釈の限界と次の判断

**段階1の候補条件は満たした。** その時点の携帯接続で、保持した同じ送信元endpointから二つの外部IPv4へ接続し、約50秒sourceを維持できた。

一方、次は未確認である。

- 同時刻の送信元port非固定との比較。今回の結果だけで、port固定がIP変動を止めた唯一の原因とは言えない。
- NATの外側で割り当てられたTCP port。同じ外向きIPは観測したが、外側のportは応答に含まれず、完全なEndpoint-Independent Mappingの証明ではない。
- 同じsourceをhp向けの新しい接続でも使えるか、ルーター／回線の受信制限を通過するか。
- 新規接続を繰り返す場合やTIME_WAIT、携帯再接続、長時間、通常のブラウザでの動作。
- WANでのBAN、解除、期限失効、daemon再起動後の到達。

**測定後は接続を閉じたので、今回観測した外向きIPv4を次回の許可値へそのまま流用しない。** 次のWAN候補は、接続を保持して現在値を確認した状態で、確定値への承認とhpが見た実peerの一致を確認して進める。許可は引き続きsource一件・専用TCP port一つ。到達失敗／source不一致では止め、CIDR・any・自動追従へ広げない。

今回の依頼はMac側の測定と復元まで完了。公開受入れHOLDは維持し、追加の設定変更・WAN試験をこの結果だけで自動再開しない。

## 証拠

- [再集計・独立復元確認を含む集約](../../artifacts/mac-mobile-fixed-port-43e65hss/summary.json)
- [原測定結果](../../artifacts/mac-mobile-fixed-port-43e65hss/result.json)、[時系列](../../artifacts/mac-mobile-fixed-port-43e65hss/events.jsonl)
- [変更前](../../artifacts/mac-mobile-fixed-port-43e65hss/before.json)、[実行・復元](../../artifacts/mac-mobile-fixed-port-43e65hss/run-result.json)、[別コマンドの終了確認](../../artifacts/mac-mobile-fixed-port-43e65hss/independent-post.json)
- [測定helper](../../artifacts/mac-mobile-fixed-port-43e65hss/probe.py)、[一時Wi-Fi変更と復元wrapper](../../artifacts/mac-mobile-fixed-port-43e65hss/run.py)。SHA-256は集約に保持。既存成果物の上書きなし、新規private directoryに保存。

製品コード変更はなく、製品の回帰試験を再実施したとは報告しない。保存結果の再集計と文書検査を行い、[公開前チェックリスト](../local-ban/PILOT_CHECKLIST.md)のWAN欄は未チェックのままとする。
