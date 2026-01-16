#!/bin/bash
set -e

echo "===================================="
echo "Building LrGenius Application for Mac"
echo "===================================="
echo ""

# Step 1: Build the geniusai-server
echo "[1/4] Building geniusai-server..."
cd ../geniusai-server
./build.sh
echo ""

# Step 2: Prepare the model bundle
echo "[2/4] Preparing CLIP model bundle..."
# python prepare_model_bundle.py
echo " skipped (not needed currently)"

# Step 3: Build the Lightroom plugin
echo "[3/4] Building Lightroom plugin..."
cd ../LrGeniusAI
./build.sh
echo ""

# Step 4: Summary
echo "[4/4] Build complete!"
echo ""
echo "===================================="
echo "Build Summary"
echo "===================================="

