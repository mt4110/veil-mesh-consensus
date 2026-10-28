# nftablesの権限境界・停止契約のオフライン試作

**採否更新（2026-10-27）:** 現行BOOTTIME絶対期限を維持する標準nftablesへの移植は[不採用](../../docs/review/2026-10-27-NFT-BOOTTIME-ADOPTION-DECISION.md)。このcrateと全ての検証記録は保全するが、製品向けadapter実装・追加試験へ自動的に進まない。

**本番binaryではありません。nftablesを実行せず、kernelやホストへ接続しない、依存なしのRustモデルです。** `--apply`・引数なしはexit 2。既存XDP、BFT、WAL、workspace／lockfileに組み込まない独立crateです。unsafeコードを禁止しています。

今回の対象は設計とオフライン検証までです。実機へのcapability付与、Netlink接続、SO_PEERCRED、namespace作成、nftの構文・kernel受理、packet、SIGKILL実験、sudo／SSH／cloudは実施しません。

## 実行

repository rootから、既存のcargoとPythonで実行します。新しいprivate evidenceとその配下のbuild先を作り、既存製品七ファイルのhashを前後比較します。

```sh
python3 scripts/nft-boundary-offline-check.py
```

runnerはformat確認、`cargo test --offline --locked`、Clippy、demo、適用コマンド拒否を行います。root権限を要求しません。生成物は`artifacts/nft-boundary-check-*/`へ保全し、自動削除しません。

## 何を検証するか

- 40 byte固定frame。IP・有限TTL・操作・instance・revisionだけ。任意command、table、path、namespace、宛先portを入力で指定できません。
- 設定されたpeer UID・namespace・owner socketとの一致。不正入力、保護IP、TTL0、容量、古い操作を拒否します。ただし観測credential自体はモデル入力で、実OSの認証を検証してはいません。
- mutationは一件だけ、待ちqueueなし、5変更/秒。結果不明でQuarantinedへ入り、後続mutationと自動再送を止めます。
- ACK前後の障害と、長い旧BANを忘れない保留状態。owner終了→table不存在確認→新instanceの順序をモデル化します。
- 保持256件とuserspace掃除16件/回。参考kernelの絶対期限と、実際のnft TTLを区別します。kernel側のGC時間や容量回収は未検証です。
- 相対TTLへの素朴な置換による期限延長と、suspendを含む時計差の反例。

`ReferenceKernel`はテスト内のメモリ実装です。packetのDrop／Pass、ownerソケット終了、SIGKILLの代わりのイベントもシミュレーションであり、実nftablesの成功として数えません。

## 採否ゲート

demoは常に`apply_supported=false`／`deployment_gate=BLOCKED`を出します。実OSの権限・所有table・通信・復旧に加え、**現行CLOCK_BOOTTIMEの絶対期限と同等の保証が未確認**なためです。成功テストが増えても、このゲートを解除しません。

設計は[権限分離・停止復旧・期限の契約](../../docs/local-ban/NFT_BOUNDARY_DESIGN.md)、今回の結果は[実施記録](../../docs/review/2026-10-27-NFT-BOUNDARY-OFFLINE.md)を参照してください。実機確認は別の具体的承認を要し、現在このcrateに実適用adapterはありません。

後続では現行絶対期限の維持を選択し、BAN適用を保留しました。別のPython probeで[owner機能だけの隔離確認](../../docs/local-ban/NFT_OWNER_PROBE_PLAN.md)を準備し、本人承認後に[hp-ubuntuで一回の空table回収・保全確認](../../docs/review/2026-10-27-NFT-OWNER-ISOLATED-ACCEPTANCE.md)を完了しました。このRustモデルや製品への適用機能追加ではありません。
