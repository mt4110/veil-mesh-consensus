# コンセンサス・eBPF関連のシナリオ試験と制約

[README](../README.md) · [確認状況と根拠](VALIDATION_STATUS.md) · [セキュリティ上の制約](../SECURITY.md)

2026-10-27の公開コード確認に基づく説明です。`proof_1`〜`proof_14` は関数名であり、数学的証明・本番合格・全環境対応を意味しません。**今回、製品試験は再実行していません。** テストが存在することと、指定環境で実行・合格したことを分けます。

## 1. 実装構成

Domainにコマンド・QC・方針、UseCaseに状態機械、Adapterに署名・wire型、InfraにQUIC・bridge・TUIがあります。構造を分離したこと自体で安全性は証明されません。[型](../veil-node/src/domain/entities.rs)、[状態機械](../veil-node/src/usecase/consensus_engine.rs)、[QUIC](../veil-node/src/infra/quic_mtls.rs)、[bridge](../veil-node/src/ebpf_bridge.rs)を参照してください。

## 2. `proof_1`〜`proof_14` の対象

以下は[公開テストソース](../veil-node/src/proof_tests.rs)を読んで確認したassertionの範囲です。

### シナリオ 1：選挙の状態遷移

- 関数：`proof_1_election_safety_and_log_up_to_date_check`
- 検査する範囲：同一プロセス内で特定の投票順序を検査。
- 結果から言えないこと：任意の故障・再起動順序に対する選挙の証明ではない。

### シナリオ 2：loopback mTLS QUIC

- 関数：`proof_2_three_node_quic_consensus_and_ebpf_xdp_enforcement`
- 検査する範囲：同一OSプロセス内の4状態機械・実UDPソケットでQCと `check_xdp_packet` を検査。
- 結果から言えないこと：実kernelへpacketを送る試験でも独立4ホストの受入れでもない。

### シナリオ 3：分断を模した配送

- 関数：`proof_3_network_partition_split_brain_prevention_and_recovery`
- 検査する範囲：状態機械へ届けるRPCを制御し、少数派・再配送を検査。
- 結果から言えないこと：実ネットワーク分断や全BFTシナリオの保証ではない。

### シナリオ 4：Mock LSM・消去

- 関数：`proof_4_ebpf_kernel_bridge_and_lsm_stealth_gc_zeroization`
- 検査する範囲：`MockEbpfBridge` のpolicyとダミーbufferを検査。
- 結果から言えないこと：実kernel LSM・実秘密鍵・全コピーの消去ではない。

### シナリオ 5：不正署名RPC

- 関数：`proof_5_byzantine_fault_tolerance_ed25519_signature_verification`
- 検査する範囲：限られた署名者詐称・偽造・改ざんを検査。
- 結果から言えないこと：任意の侵害ノードによる正規署名の悪用まで証明しない。

### シナリオ 6：TUI process

- 関数：`proof_6_tui_arena_mock_attack_process_spawn_and_extermination`
- 検査する範囲：同一プロセスTUIと試験所有processの停止を検査。
- 結果から言えないこと：自動攻撃検知・任意processの安全な停止ではない。

### シナリオ 7：条件付き実kernel veth

- 関数：`proof_7_live_linux_kernel_xdp_drop_on_veth`
- 検査する範囲：object・veth・権限が揃えばpingのPASS→DROP→PASSを検査。
- 結果から言えないこと：前提不足／attach失敗でも `Ok(())`。全testのPASSだけで実行済みと判断できない。

### シナリオ 8：特定の二枚舌・QC不足

- 関数：`proof_8_leader_equivocation_and_multisig_quorum_certificate_defense`
- 検査する範囲：署名付きRPC入口で特定の競合・証明不足を検査。
- 結果から言えないこと：本番用の完成BFTや全viewでの安全性の証明ではない。

### シナリオ 9：TLS・wire拒否

- 関数：`proof_9_network_quic_mtls_and_signed_rpc_rejects_untrusted_tls_and_forged_wire_packets`
- 検査する範囲：loopback QUICで未信頼TLSや偽造RPCの拒否を検査。
- 結果から言えないこと：安全な実環境PKI配布・独立host運用を保証しない。

### シナリオ 10：match_index・署名数

- 関数：`proof_10_lying_follower_match_index_cannot_trigger_two_node_commit`
- 検査する範囲：限定した不正応答と4状態機械の定足数を検査。
- 結果から言えないこと：署名数だけで決定の一意性や入力の真実性は証明しない。

### シナリオ 11：境界・鍵・保護IP

- 関数：`proof_11_boundary_validation_index_zero_and_term_max_no_panic_or_saturation`
- 検査する範囲：index／term、名前由来の既知seed、exact loopback等のcaseを検査。
- 結果から言えないこと：全境界、全loopback／管理網／gateway保護の網羅確認ではない。

### シナリオ 12：timeout明示駆動

- 関数：`proof_12_v0_4_view_change_timeout_certificate_rotates_leader_on_censorship`
- 検査する範囲：`trigger_view_timeout()` を試験から呼び、TCと次termを検査。
- 結果から言えないこと：通常daemonのtimer、SIGSTOP、独立processの検閲回復を試験していない。

### シナリオ 13：TTL・batch・bypass・metrics

- 関数：`proof_13_v0_5_ttl_auto_expiry_micro_batch_emergency_bypass_and_prometheus_metrics`
- 検査する範囲：batchを直接提案し、論理時計を進め、ユーザー空間の判定と文字列を検査。
- 結果から言えないこと：実時計・kernel TTL、50ms集約処理、kernel bypass、HTTP exporterの受入れではない。

### シナリオ 14：複合状態機械case

- 関数：`proof_14_v0_9_1_adversarial_chaos_partition_byzantine_flood_and_wal_recovery`
- 検査する範囲：RPC手動配送、明示WAL保存／別stateへ復元、timeout呼出し、再配送を検査。
- 結果から言えないこと：tc netem・独立host障害・電源断・実kernel・任意BFT攻撃の網羅試験ではない。

## 3. 回帰テストと過去の反例

同じソースにはprefix hash、投票lock、PKI、apply error、鍵mode、catch-up件数、term上限、WAL等の回帰caseがあります。個々のassertionは有限の入力・配送順序を扱い、R1〜R11の全契約や本番運用が完成したことを意味しません。

[c8d3f7fの再現記録](review/2026-10-23-REPRODUCTION.md)は古い版の欠陥をassertする試験です。その成功は安全性の確認とは逆の意味です。後続公開版の修正済み部分を、その旧反例だけから未修正と断定することもしません。

## 4. 実kernel・CI・scriptの判定

`proof_7` のobject不在、veth不在、attach失敗は成功扱いのreturnです。CIのgreen表示、実行件数、テスト名から実kernel実行を推定しません。実行を主張するには対象commit／object／kernel／interfaceと、実packet・counter・停止復帰等の証拠を別に確認する必要があります。

`arena-smoke.sh` はcargo出力を保存した後、固定のPASSと `proof_1`〜`proof_6` のリストを要約へ書きます。実行caseを解析してskipを区別する集計ではなく、Arena全体の認証・互換性試験でもありません。

`check.sh` はShellCheckが見つからなければその検査を実行しません。workspace試験は、除外されているeBPF crateやC版objectをすべて再buildして検証する手順ではありません。

```bash
# 開発当時の研究用チェック。各条件付き検査の実行範囲は個別確認
bash scripts/check.sh
```

`deploy-pi-node.sh` は専用hostのnetwork・trace等を変更し得る過去のscriptです。この文書監査では実行していません。製品コード・テストの安全条件を弱めて文書へ合わせる変更もしていません。
