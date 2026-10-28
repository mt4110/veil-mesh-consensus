# ノード別TLS証明書・鍵の運用契約

対象: 通常daemon、署名protocol v3、WAL v4。2026-10-24（Asia/Tokyo）。

最終ゴールは小規模サイトで安全に試験導入できる状態にすること。今回のマイルストーンは、共有CA秘密鍵をdaemonへ配る方式をやめ、ノード別のTLS認証・発行・更新・失効を扱えるようにすること。実装、回帰、pi-node上の4プロセス試験を完了した。複数ホストへの本番配布や既存CA秘密鍵の削除は行っていない。

## 境界と形式

通常daemonは `--tls-dir` で指定された既存bundleだけを読む。CA生成・署名、TLS鍵の欠落時の自動生成、既定の共有CAへのfallbackはない。以前の `ClusterMtlsPki` と汎用 `veil.mesh` endpointはテスト専用に制限し、`rcgen` をdev-dependencyへ移した。daemonの合意署名鍵とTLS秘密鍵は別物である。

bundleは実directory、mode 0700。次の4ファイル以外があれば拒否する。秘密鍵は0600、全入力はregular file、symlinkやgroup/other writableを拒否し、読込サイズにも上限を設ける。

| ファイル | 内容 |
| --- | --- |
| `ca.cert.der` | 信頼するCA公開証明書1枚。DER、最大16KiB |
| `node.cert.der` | ノードleaf証明書1枚。DER、最大16KiB |
| `node.key.der` | ノードTLS秘密鍵。PKCS#8 DER、0600、最大16KiB |
| `revoked.txt` | 失効対象leafのSHA-256 fingerprint。1行64桁hex、最大4096件。空ファイルも明示的に必要 |

直接rootが発行するleafだけを扱う。中間CA chainや複数rootのtrust bundle、OCSP、X.509 CRLのprofileではない。`revoked.txt` は管理者が信頼できる経路で配布するローカル拒否リストであり、CA署名付きCRLではない。

TLS一式は起動時に読み込み、再読込しない。新しい世代directoryを作り、検査後に `--tls-dir` の参照先を変更して再起動する。途中まで書けたbundleを使用したり、既存ファイルを上書きしたりしない。

`--cluster-dir` と旧 `VEIL_CLUSTER_PKI_DIR` 環境変数は、既存の署名鍵/WAL既定pathを突然変えてしまわないため、state pathとしての互換性を残した。TLSの入力元ではない。移行後は `--key-file`、`--wal-file`、`--tls-dir` を明示することを推奨する。

## TLS identityと合意identityの対応

SANのDNS nameを次の形に固定する。DNS問合せには使用しない。

```text
n<node IDのSHA-256前半32桁>.n<後半32桁>.c<config digest前半32桁>.c<後半32桁>.veil.invalid
```

node IDを直接DNS labelにしないため、長いIDやunderscoreを曖昧に変換せず扱える。config digestはcluster ID、epoch、全memberと合意公開鍵、接続先、保護方針、時計設定を含む既存の値である。CAが信頼されるだけでは別ノード・別構成の証明書を使えない。

- 起動時に、自分の証明書のchain、時刻、有効なserver/client両用途、SANと設定node ID、TLS鍵の一致、失効リストを検査する。WAL作成・XDP attach・socket作成より前に実施する。
- 接続先は、固定member IDから求めたSANで標準のrustls server認証を受ける。接続後にも証明書・失効を確認し、応答署名者がそのpeer IDであることを検査する。
- 受信側はmTLS client認証に加え、各合意RPCの署名者IDがその接続の証明書SANに対応することを確認してから、既存の署名・context・QC・WAL処理へ渡す。
- 既存connection上でも各RPCごとに時刻と失効を検査する。受信handshake、stream受付・読込にはdeadline、同時接続には上限128を設けた。負荷性能の保証ではない。
- 通常daemonのTLS経路では、認証済み合意RPCを扱い、従来の汎用Pingを運用認証の代用にしない。

TLS証明書やrootのfingerprint自体は合意config digestへ追加していない。期待するTLS identityを既存digestから導出することで、TLS鍵・leafの更新を合意鍵変更やepoch移行と分離する。構成digestが変わる場合はSANも変わるため証明書の再発行が必要だが、それでWAL/epoch移行が実装されたことにはならない。

## 発行手順

発行ツールは `scripts/tls-pki.py`。Python 3とOpenSSL 3以降を使う。Rust側の新しい実行時依存は追加していない。各操作は既存出力への上書き・削除をせず、エラー時も生成途中の資料を保持する。

以下の `$TLS_NAME` は、対象ノードの**承認済み固定構成**に対して求める。通常daemonと同じ `--node-id`、`--listen`、`--cluster-id`、`--config-epoch`、全 `--member`、追加保護IP、時計設定を指定し、`--print-tls-name` を付ける。このモードはTLS鍵・合意秘密鍵を読まずに終了する。

### 1. 管理環境でCAを新規作成

```sh
python3 scripts/tls-pki.py init-ca --out ./offline-ca-2026 --days 365
```

このdirectoryは管理環境に留める。CA秘密鍵は暗号化されていないPKCS#8 DERを0600で保存するため、管理ホスト・暗号化保管・backupの扱いを別途決める。daemonホストへ配るのはCAの公開証明書だけである。以前すべてのノードに配ったCA秘密鍵は、今回の新方式のrootとして再利用しない。

### 2. 各ノードでTLS鍵とCSRを作成

```sh
python3 scripts/tls-pki.py request --out ./node-a-request-01
```

`node.key.der` はそのノードに留め、`node.csr.der` だけを管理環境へ転送する。合意署名用 `.pk8` をこの操作で作り直さない。

### 3. 管理環境で承認したnode/configへ発行

```sh
python3 scripts/tls-pki.py issue \
  --ca-dir ./offline-ca-2026 \
  --csr ./received-node-a.csr.der \
  --tls-name "$TLS_NAME" \
  --out ./node-a-issued-01 --days 30
```

CSRの署名を検証し、承認したSANを明示設定する。CSRが要求したextensionはコピーしない。requestコマンドはP-256鍵を生成する。issueコマンドはleafに `CA:FALSE`、DigitalSignature、serverAuth/clientAuth、exact SANを設定する。leaf有効期間は既定30日、指定範囲1〜90日。CA残存期間がそのleaf期間と余裕60秒を満たさない場合は拒否する。

管理者は `--tls-name` と承認済み構成の対応を確認する。CSRだけで発行権限を自動承認する仕組みではない。発行先directoryには公開証明書、検証用公開PEM、identity/fingerprintの記録を保持する。ノード秘密鍵は受け取らない。

### 4. ノードでbundleを組み立て、起動前に検査

公開の発行結果と管理者が配布する最新失効リストをノードへ戻す。初回だけは空の `revoked.txt` を使える。更新時に空リストへ戻さない。

```sh
python3 scripts/tls-pki.py bundle \
  --request-dir ./node-a-request-01 \
  --issued-dir ./received-node-a-issued-01 \
  --revoked ./received-revoked-current.txt \
  --out ./node-a-tls-01
```

bundle処理で証明書と秘密鍵の公開鍵一致、失効fingerprintを確認する。最後に通常daemonと同じ固定構成引数へ `--check-tls --tls-dir ./node-a-tls-01` を追加して検査する。`--check-tls` はchain・SAN・用途・有効期限・鍵・失効を検査し、公開fingerprintだけを出力して終了する。合意署名鍵やWALを読み書きせず、listenerも開かない。

検査に成功したら、通常daemonの起動引数に `--tls-dir ./node-a-tls-01` を指定する。既存の合意署名鍵、WAL、固定member構成を保持する。起動ログに出る証明書SHA-256が検査結果と一致することを確認する。

## 更新・失効・障害時の操作

### leaf/TLS鍵の更新

1. 同じnode ID・固定構成に対して、新しいrequest directoryで新しいTLS鍵とCSRを作る。
2. 管理環境で新しい証明書を発行する。
3. 最新失効リストを使って新しいbundleを作り、`--check-tls` する。
4. 定足数を維持できる範囲でノードを一台ずつ再起動し、公開fingerprint、leader、commit/applyとcatch-upを確認する。
5. 不要になった旧leafを失効リストに追加し、全検証ノードへ配布・再起動する。証明書を再発行しただけでは旧証明書は失効しない。

同じCA、node ID、config digestであれば、TLS更新で合意署名鍵・WALを変更しない。証明書更新のために合意epochを上げない。定足数を維持できない小構成では保守停止が必要になる。

### 旧leafの失効

```sh
python3 scripts/tls-pki.py revoke \
  --certificate ./old-node-a.cert.der \
  --previous ./revoked-current.txt \
  --out ./revoked-next.txt
```

既存の失効一覧を保持して追加した新規ファイルを作る。各ノードでこの一覧を含む新bundleを作り、再起動して適用する。現在の自分のleafが失効対象なら起動を拒否するため、そのノードは新しいTLS鍵・証明書も用意する。

**失効は未更新ノードへ即時伝播しない。** 自動配布、hot reload、単調世代による古い失効リストへのrollback防止は今回の機能ではない。緊急時は、未更新ノードや侵害ノードを別の管理経路で通信隔離し、全検証者への反映を確認する必要がある。これは運用上の必要条件であり、今回その隔離操作を実施したという意味ではない。

同じ秘密鍵で複数leafを発行済みなら、漏えい鍵に対応する全leafを確認して失効させる。TLS失効は合意member除外や合意署名鍵の失効ではなく、過去のQCを遡って無効化しない。合意署名鍵も漏えいした場合、未実装の安全なmembership/epoch移行を回避して継続してはならない。

### CA鍵漏えい・CA更新

CA秘密鍵漏えいは、leaf単位の拒否だけでは封じられない。新しいoffline CAと全ノードleafを用意して、trust anchorを変更する。このprofileはrootを1枚しか受け付けず、新旧rootが混在する間の無停止運用を保証しない。定足数と停止計画を用意する。自動root rotationは実装していない。

CA公開証明書はtrust anchorとして扱う。標準rustlsのanchor処理にrootの有効期間監視を任せず、管理側で期限を管理する。発行ツールはleafがCA期限を越えないよう残期間を検査する。外部発行証明書を持ち込む場合も同じprofileを満たすことを管理側で確認する。

```sh
python3 scripts/tls-pki.py inspect --certificate ./node-a.cert.der
python3 scripts/tls-pki.py inspect --certificate ./ca.cert.der
```

期限・SAN・fingerprintを確認できる。自動更新・期限通知は含まない。期限切れや誤配置を隠すfallbackもない。TLSの時刻判定はOS時計を前提とする。BAN leaseの時計品質検査とは別であり、OS時計やNTPはこの変更で操作しない。

## 検証結果と範囲

| 検証 | 結果 |
| --- | --- |
| macOS通常回帰 | 81成功、0失敗、1 ignored |
| pi-node通常回帰 | 80成功、0失敗、4 ignored、3 filtered |
| 両環境all-target Clippy、ローカルfmt/diff/Markdown | 成功 |
| TLS bundle異常系 | 別node、別cluster/epoch/digest、鍵不一致、欠落、symlink、権限、巨大入力、不正/欠落/失効リスト、CA秘密鍵混入、期限切れ/未来、用途不足を拒否 |
| 実mTLS QUIC | 同じCAのC証明書でAの正常署名を再送しても拒否し、WAL不変。誤った接続先identityも拒否。正しいidentityは成功 |
| TLS失効と更新 | 旧leafを実QUICで拒否しWAL不変、新しいleafで正常に投票要求を受理 |
| 発行/起動CLI | CSR→offline発行→bundle→事前検査、上書き拒否、鍵不一致・過長期限等を拒否。6種の不正daemon起動はstate/socket作成前に拒否 |
| 従来のLinux実試験 | 期限付きcatch-up、SIGSTOP失効、WAL復旧、bypass、未登録署名者拒否が成功 |
| 新方式で通常daemon 4プロセス | 選挙、BAN、実packet DROP→期限後PASS、再選出、WAL再起動、少数側pending、期限切れcatch-upを確認 |
| TLS更新後の4プロセス | 1ノードのTLS鍵/leaf更新と全台への旧leaf失効一覧反映後、合意署名鍵不変・WAL commit維持・新規BAN合意/失効が成功 |

Linux通常回帰のfilteredは従来どおり `proof_6_tui_arena`、`proof_7_live_linux`、`regression_r6_r7_linux`。ignoredを一括して合格扱いせず、実行した専用試験を別記した。macOSだけの追加1件は通常daemonの時計fallback拒否テスト。

受入試験は `scripts/tls-pki-check.py` と更新した `scripts/daemon-flow-check.py`。いずれもPython標準ライブラリを使う。OpenSSLはmacOS 3.6.4、pi-node 3.5.5で確認した。通常daemonの実行にはOpenSSL CLI/Pythonは不要である。

```sh
cargo test --workspace --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo build --workspace --locked --offline
python3 scripts/tls-pki-check.py \
  --binary target/debug/veil-node --output-dir /tmp/veil-pki-new-run
# rootと既存の隔離veth-victim/ns-veil-rogueが必要。既存XDPがあれば拒否する。
sudo python3 scripts/daemon-flow-check.py \
  --binary target/debug/veil-node --output-dir /tmp/veil-tls-new-run \
  --xdp assets/veil_xdp_kern.o
```

pi-nodeでは同一Linuxホストの4プロセス、loopbackのQUIC、1ノードだけに既存の隔離veth XDPを設定した。発行者とdaemonは別directoryで、各daemon bundleにCA秘密鍵がないことを検査した。**物理的にofflineな別ホストへの配布、HSM、rootユーザーからCAを隔離するOS権限境界を証明した試験ではない。** 本番ではCAをdaemonホストに置かないことが運用条件となる。

証拠:

- ローカル: `artifacts/node-tls-17l38grq/`
- pi-node隔離source: `/tmp/veil-tls-gfRDrWMg`
- 最終通常daemon結果: `/tmp/veil-tls-gfRDrWMg/flow-final/result.json`
- 発行/起動検査: `/tmp/veil-tls-gfRDrWMg/pki-final/result.json`
- source snapshot、差分、検査ログ、公開fingerprint、WAL、結果JSONを保存する。回収archiveへ秘密鍵を含めない。
- 試験終了後に所有daemonが終了し、XDP残留なし・ping復帰を確認する。物理NIC、host時計、NTP、既存firewallは変更しない。fixtureや秘密鍵の自動削除はしない。

## 残る投入条件

この変更で、通常daemonへ共有CA秘密鍵を配布する必要はなくなった。安全なmembership/epoch移行、敵対的なBFT view-change、複数ホストでの運用受入れ、導入先の管理経路保護と容量・過負荷対策は別の課題として残る。TLS配布・再起動手順の実運用者による確認も必要であり、本番自動遮断の保留を解除するものではない。

実装時は固定依存rustls 0.23.46のソースを参照した。発行CLIの根拠は[OpenSSL req（CSR）](https://docs.openssl.org/3.0/man1/openssl-req/)と[OpenSSL x509（CAによる署名とextension指定）](https://docs.openssl.org/3.0/man1/openssl-x509/)。CSR extensionを自動コピーせず、発行者が承認したidentityを指定する。

## 後続のWAL初期構築変更

通常起動は正常な既存WALを必須とし、初回の空state作成は `--init-key` が発行する一度限りの許可と `--init-state` へ分離した。新規クラスタの起動例にはこの操作が追加で必要。既存鍵で欠損WALを再作成する操作ではない。[起動契約・互換性・復旧の境界](2026-10-24-WAL-BOOTSTRAP-RECOVERY.md)を優先する。
