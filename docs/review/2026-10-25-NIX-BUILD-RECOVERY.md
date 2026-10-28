# pi-nodeのNixパスと再ビルド・回帰検証の復旧

2026-10-25 JST。**既存データの所在を確認し、追加のホスト再起動・永続設定変更なしで、新規ビルドとCargo経由の回帰検証に成功した。**

最終ゴールは小規模公開サイトで安全に試用できる無償OSS。今回は、前回のホスト再起動後に見えなくなったNixパスを調べ、既存データを使って再ビルドできることを確認する節目。
復旧したのは下記のビルド専用セッションであり、host全体へNixを常設した状態ではない。

## 原因とデータの所在

既存の[scripts/deploy-pi-node.sh](../../scripts/deploy-pi-node.sh)の30–32行は、次のdirectoryを`/nix`へ一時的にbind mountする方式だった。

```text
/home/maakie/veil-arena-validation/run-1791624441124999000/results/kvm-runtime-copy/nix
```

再起動後はhost側の`/nix`は空の通常directoryで、mountされていなかった。一方、上記の`store/`、必要なCargo/rustcは`/dev/sda5`のext4上に残っていた。主要2実行ファイルのinode/hashも採取した。
`/etc/fstab`と検査した`/etc/systemd/system`内のunit/configには復元設定がなく、`nix.mount`もnot-found。**データ消失ではなく、一時bind mountの接続が再起動で失われた状態と判断した。**

コピー内にはNix DB・daemon socketがなく、hostにもnix-daemon unitは存在しない。この作業でフルのNix管理環境を新規導入したり、DBを再生成したりはしていない。GC・optimise・store削除も実行していない。
旧deploy scriptにはnamespace削除など別の処理もあるため、復旧のためにそのscript全体を実行していない。

## 採用した復旧方法

[pi-node-nix-rebuild.py](../../scripts/pi-node-nix-rebuild.py)を追加した。通常のUID/GID1000から実行し、固定の既存ソース・vendor・runtime copyを材料にする。

1. 新しいprivate workspaceを作り、Rust/Cargoのソース73ファイルをコピーする。旧targetやfixtureを上書きしない。
2. 一時systemd unitのmount namespace内だけに、既存runtime copyを`/nix`としてread-only bindする。
3. UID/GID1000、CapEff/Prm/Inh/Amb=0、NoNewPrivs=1でCargoを実行する。ネットワークは分離し、vendorはread-only、Cargoはoffline/locked。
4. HOMEは隠し、必要な新workspaceとvendorだけを公開する。認証情報のdirectoryと/runのDocker socketが見えないことを確認する。生成物・一時file・cacheの書込み先は新workspaceに限定する。
5. 終了後に一時unitを回収し、host側のmount・既存データ・他サービスを照合する。

insideの`/nix`は`ro,nosuid,relatime`、mount namespace inodeは呼出し元と別だった。補助groupは通常のmaakieアカウントのものを引き継ぐが、runtime capabilityは0で、Docker socketは見えない。全groupを消したという主張はしない。
外側の`/nix`は作業前後で同じ空directoryのまま。fstab、常設unit、既存unit設定、アカウントを変更せず、追加のrebootも行っていない。

## 実行結果

| 項目 | 結果 |
| --- | --- |
| Cargo | `cargo 1.98.0 (797e8a9bc 2026-08-05)` |
| rustc | `rustc 1.98.1 (48a229cea 2026-09-01) (built from a source tarball)` |
| 新規build | `cargo build --offline --locked --jobs 2 -p veil-node --bin veil-local-ban`成功 |
| Cargo経由の回帰 | `cargo test --offline --locked --jobs 2 -p veil-node --lib`成功。**24 passed / 0 failed / 1 ignored** |
| ignored | 従来の明示的な初期化SIGKILL用fixture。今回追加で除外したものではない |
| 出力先 | `/home/maakie/vnr-a4dsem9l/target/debug/veil-local-ban`。新しいtargetで実際にコンパイルした |
| source | native側からコピーした73ファイルがlocal checkoutと一致。build/test後も旧source・コピー先sourceともhash一致 |
| runtime | コピー内Cargo/rustcのhash/inodeが作業前後で一致。build中のstoreはread-only |
| 一時unit | `veil-nix-build-a4dsem9l.service`。終了後はnot-found/MainPID0 |

一時namespace終了後、host側に/nix/storeが存在しない状態でも、新binaryの`--help`起動は成功した。これはCLI初期化の確認であり、新binaryのkernel適用試験ではない。旧native binary/test binaryも過去の記録のhashと一致した。

buildは約86.3秒、test binaryのコンパイルを含む回帰工程は約12.9秒。これは当該実行の参考値であり、性能比較やdeadline緩和の根拠にはしない。
前回は既存test binaryの直接実行だったが、今回は新targetへのコンパイルとCargo経由の回帰を実施できた。

製品Rust/BFT/WAL保存方式・sync_all・TTL上限・時間制限は変更していない。256件上限と新instanceの揮発契約を含むlibrary回帰を維持した。
配布済み/optのbinaryを新binaryへ交換しておらず、追加のkernel/XDP適用試験やホスト再起動は行っていない。前回の実boot確認を今回新たに実施したとは扱わない。

上記はこのビルド復旧時点の記録。その後、同じ新binaryの実体hashを確認しながら[実kernelでの隔離受入れ](2026-10-25-REBUILT-LOCAL-BAN-KERNEL-ACCEPTANCE.md)を実施した。旧/optのbinary交換や追加のhost rebootは行っていない。

## 保全確認

- test/fullの既存fixtureは、metadataとfile hashが一致した。
- Docker/mainnetを含む主要サービスのunit本体/drop-in hash、状態・PIDが一致。mainnetのcontainer IDも今回は変化せずrunningだった。
- host interfaceの名前/ifindex/MAC/MTU/flags/XDP、fstab hash、boot ID、passwd/group entry hashも一致。
- boot/namespaceの試験用unitは停止・disabledのまま。既存full/test unitも起動していない。
- hostの/nix directory inode・空の内容・非mount状態が一致。一時unitのmountをhostへ伝播させていない。
- これらは対象を絞った保全確認であり、host上の全fileやmainnetの論理的データ整合性を完全に証明したものではない。

## 再実行手順と限界

repository rootから実行する例:

```sh
# --applyなしは既存依存の読取り確認だけ
ssh pi-node 'python3 -' < scripts/pi-node-nix-rebuild.py

# 新しいworkspaceを作り、一時セッションでbuildとlibrary回帰
ssh pi-node 'python3 - --apply' < scripts/pi-node-nix-rebuild.py
```

実行には既存のsudo権限が必要。scriptは非対話sudoに失敗した場合も、恒久設定や権限を変更せず止まる。
毎回新規workspaceを作り、失敗時も証拠を残す。自動掃除はしない。この復旧scriptは固定済みnative作業コピーを検証するもので、localの未同期の編集を自動転送するものではない。

元のruntime copy directoryは、このビルド手順の依存先として保全する。見た目が過去のvalidation結果でも、削除や移動の前には別途移行計画と検証が必要。
hostの通常SSH shellで直接`/nix/store/...`を実行できる恒久mountは設定していない。フルの`nix develop`/package管理やBFT全体の回帰も今回の完了範囲ではない。

## 証拠

- local: `artifacts/nix-rebuild-im0mm450/`
- [worker実行結果](../../artifacts/nix-rebuild-im0mm450/worker-result.json)、[保全確認](../../artifacts/nix-rebuild-im0mm450/preservation-checks.json)、[build log](../../artifacts/nix-rebuild-im0mm450/build.log)、[Cargo回帰log](../../artifacts/nix-rebuild-im0mm450/regression.log)
- before.json/after.json、run-manifest.json、source-manifest.json、runtime-provenance.json、launcher-result.json、transient-unit-final.txtも同じprivate領域へ保存。
- remote source/output/evidence: `/home/maakie/vnr-a4dsem9l/`。既存のsource workspaceとruntime copyは保全した。
