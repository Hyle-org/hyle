#!/bin/bash
echo "Press any key to continue when asked"
echo ""
echo ""
cat ./contract_c1.ron
read -p "Send Contract C1 ?  " -n1 -s
echo ""
../target/debug/client --config-file ../tests/node1/conf.ron send contract ./contract_c1.ron

echo ""
echo ""
echo ""
cat ./contract_c2.ron
read -p "Send Contract C2 ?" -n1 -s
echo ""
../target/debug/client --config-file ../tests/node1/conf.ron send contract ./contract_c2.ron

echo ""
echo ""
echo ""
cat ./tx1_blob.ron
read -p "Send Blob tx ?" -n1 -s
echo ""
../target/debug/client --config-file ../tests/node1/conf.ron send blob ./tx1_blob.ron

echo ""
echo ""
echo ""
cat ./tx1_proof.ron
read -p "Send Proof tx ?" -n1 -s
echo ""
../target/debug/client --config-file ../tests/node1/conf.ron send proof ./tx1_proof.ron
