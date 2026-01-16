#!/bin/bash

echo "===================================="
echo "Cleaning LrGenius Build Artifacts"
echo "===================================="
echo ""
echo "This will remove all build outputs and temporary files."
echo ""
#read -p "Press Enter to continue..."

echo "Cleaning geniusai-server build artifacts..."
cd ../geniusai-server
rm -rf build dist models/bundle __pycache__ src/__pycache__
cd ../LrGeniusAI

echo "Cleaning LrGeniusAI build artifacts..."
rm -rf build dist

echo ""
echo "===================================="
echo "Clean complete!"
echo "===================================="
echo ""
echo "Build artifacts have been removed."
echo "Model cache in geniusai-server/models/ was preserved."
echo ""
echo "To rebuild, run: ./build-all.sh"
echo ""
