#!/bin/bash
#set -x


echo "Removing build directory"
rm -rf LrGeniusAI.lrplugin
echo "Creating build directory"
mkdir -p build/LrGeniusAI.lrplugin

echo "Compiling"
cd LrGeniusAI.lrdevplugin
for file in $(ls -1 *.lua) ; do
    ../compilers/luac_mac -o ../build/LrGeniusAI.lrplugin/${file} $file
done

echo "Copying translations"
cp TranslatedStrings_*.txt ../build/LrGeniusAI.lrplugin/

cd ../..
