# 2026-10-23 レビュー再現手順と証拠

対象は `c8d3f7f`。2026-10-27の公開文書訂正時点のmainを再試験した結果ではありません。[公開版との区別](../VALIDATION_STATUS.md)を参照してください。製品コードを変更せず、一時コピーに [再現ケース](2026-10-23-reproduction-cases.rs) を追加した。**これらの追加テストは、欠陥が存在することをassertする。成功は安全性の確認を意味しない。** 修正後の回帰テストにする場合は、期待する不変条件をassertする形へ書き換える。

## 保存した証拠

ローカルの証拠ディレクトリ:

```text
artifacts/review-20261023-kvbx7g1s/
```

| ファイル | 内容 |
| --- | --- |
| `source-commit.txt` / `source.tar` | レビュー対象のGit commitと追跡ファイルのsnapshot |
| `linux-original-build.txt` | 無変更の対象版HEADのLinuxテストbuild失敗。コマンドと診断抜粋 |
| `linux-proof7-qualified-copy.txt` | 検証コピーの名前参照だけ補正後、隔離vethの実pingによるPASS→DROP→PASS |
| `macos-reproductions.txt` | 元の10件と追加8件、計18件の実行ログ |
| `linux-xdp-boundaries.txt` | 最初の追加kernelテストharnessがEINVALで失敗した記録 |
| `linux-xdp-boundaries-padding-fixed.txt` | harnessのABI paddingを明示的にzeroにした後の実kernel結果 |
| `process-0.log` / `process-1.log` | 当時の対象版の独立2プロセスがそれぞれ別CAを生成し、mTLSで拒否するログ |
| `summary.json` | 環境、検証結果、変更範囲の機械可読な索引 |

最後のkernelテストは製品コードの修正ではなく、追加harnessのsyscall引数構築の修正である。最初の失敗記録も保持した。引数レイアウトとx86_64のsyscall番号はpi-nodeのLinux headerを確認した。

## macOSまたは非特権ユーザー空間の再現

Rustとロック済み依存のcacheが存在する環境を使う。以下は新しい一時ディレクトリを作り、追跡済みsnapshotを展開し、追加テストを組み込む。元リポジトリの実装を変更せず、削除処理もしない。依存がcacheになければofflineで失敗する。

```python
from pathlib import Path
import platform
import subprocess
import tempfile

root = Path.cwd()
target = Path(tempfile.mkdtemp(prefix="veil-mesh-review-"))
archive = target / "source.tar"
with archive.open("xb") as stream:
    subprocess.run(["git", "archive", "c8d3f7f"], stdout=stream, check=True)
subprocess.run(["tar", "-xf", str(archive), "-C", str(target)], check=True)
tests = target / "veil-node/src/proof_tests.rs"
original = tests.read_text()
if platform.system() == "Linux":
    # Diagnostic-only correction; the unmodified Linux build fails at this line.
    old = "ebpf_bridge::AyaKernelBridge::load_and_attach("
    assert original.count(old) == 1
    original = original.replace(old, "crate::" + old)
reproductions = root / "docs/review/2026-10-23-reproduction-cases.rs"
tests.write_text(original + "\n" + reproductions.read_text())
print("Retained review copy:", target)
subprocess.run([
    "cargo", "test", "--manifest-path", str(target / "Cargo.toml"),
    "-p", "veil-node", "--locked", "--offline", "--",
    "--skip", "proof_7_live_linux_kernel_xdp_drop_on_veth",
    "--skip", "audit_linux_xdp_source_port_and_map_capacity",
    "--nocapture",
], check=True)
```

既存のTUI試験は自身が起動したデモ用processを終了する。ほかのprocessの停止を目的とした試験ではない。

## 追加ケースの意味

| テスト名 | 再現する欠陥 |
| --- | --- |
| `audit_tail_qc_commits_conflicting_prefixes` | 同じ末尾QCで、同じIPが誠実BでDROP、誠実CでPASS |
| `audit_hidden_qc_allows_conflicting_commits_across_terms` | QCを片側へだけ配送した後、別termの競合値もcommit |
| `audit_term_sandwich_obtains_two_votes_for_same_term_and_index` | 別termを挟むと同じterm/indexへ異なる署名が出る |
| `audit_last_accepted_term_saturates_election` | 許容term上端で選挙termが増加せず、同じtermで投票先が変わる |
| `audit_catch_up_batch_exceeds_receivers_entry_limit` | 1025件のcatch-upが受信上限で拒否される |
| `audit_apply_error_is_reported_as_applied` | bridgeが失敗してもlast_appliedとメモリ上の遮断判定が進む |
| `transport_review_propose_returns_success_without_commit` | 全peer無応答なのにOk(index)、commit/appliedは0 |
| `transport_review_existing_key_permissions_are_not_validated` | 0644の既存テスト鍵を受理。秘密値は出力しない |
| `audit_linux_xdp_source_port_and_map_capacity` | 実kernelでsource port例外と256件map上限を確認 |

## Linux実機試験の条件

ユーザー指定の `ssh pi-node` を使用。環境はLinux 7.0.0-38-generic x86_64。既存checkoutへ上書きせず、`/tmp/veil-mesh-review-20261023.ZYzrw4` に対象commitを展開した。Rust 1.98.1と既存vendorを用いてbuildした。

元のLinuxテストは `proof_tests.rs:424` の `ebpf_bridge::AyaKernelBridge` が名前解決できずE0433で失敗した。検証コピーだけに `crate::` を加え、その結果を無変更の対象版HEADの成功と区別した。

`proof_7` は、事前に存在していた `veth-victim` と `ns-veil-rogue` を使用。既存attachがないことを確認してから、試験所有のXDP linkで実pingの疎通・拒否・解除を確認した。実NIC、経路、既存firewall、namespaceの構成は変更していない。試験終了後、`bpftool net` のXDP一覧が空で、テスト用namespaceからのpingが成功することを確認した。

追加kernel試験は `Ebpf::load_file` とprogram load、`BPF_PROG_TEST_RUN` のみを使い、interfaceへのattach、pin、攻撃packetの外部送信を行わない。x86_64専用syscall番号321を使うため、別architectureではこのテストを実行しない。BPF権限を持つ検証環境で、対象テスト1件を明示して実行する。

確認値:

```text
ordinary source port 40000 -> XDP_DROP (1)
source port 22             -> XDP_PASS (2)
source port 10023          -> XDP_PASS (2)
257th distinct map entry  -> rejected
```

object SHA-256（ローカル・pi-node一致）:

```text
90ba0784f0c2ffacac1bcdf5c50903db5627acaaa7772777f7bd095b21c450dc
```

これは配布済み `assets/veil_xdp_kern.o` の検証であり、今回C/Rustソースからobjectを再buildして一致を証明したものではない。合成frameの結果からTCP接続成立、実負荷性能、IPv6、CIDR、TTL、複数ホスト合意を確認したとは扱わない。

## 2プロセスTLS試験

当時の対象版を一時コピーで `cargo build --locked --offline -p veil-node` し、`127.0.0.1` の別UDP portで2プロセスを起動した。両者へ相手のaddressを指定し、約3秒観測した。双方に `invalid peer certificate: BadSignature` が記録され、`Completed QUIC consensus round` は0件だった。観測後は試験が起動した2プロセスだけ終了し、回収した。

## 変更と保存の境界

製品ソース、原設計、既存テスト、Git index、既存証拠を変更していない。新規成果物はレビュー/改訂案、この手順、再現ケースである。commit・push・PR・本番反映・外部サービスへの文書送信は行っていない。一時コピーと証拠を保持し、削除していない。
