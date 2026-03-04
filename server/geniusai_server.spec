# -*- mode: python ; coding: utf-8 -*-

block_cipher = None

from PyInstaller.utils.hooks import copy_metadata, collect_data_files

# Collect metadata for packages that check their own version at runtime
datas = [('open_clip', 'open_clip')]
datas += copy_metadata('tqdm')
datas += copy_metadata('transformers')
datas += copy_metadata('tokenizers')
datas += copy_metadata('huggingface-hub')
datas += copy_metadata('filelock')
datas += copy_metadata('numpy')
datas += copy_metadata('packaging')
datas += copy_metadata('regex')
datas += copy_metadata('requests')
datas += copy_metadata('safetensors')
datas += copy_metadata('pyyaml')
datas += copy_metadata('certifi')
datas += copy_metadata('charset-normalizer')
datas += copy_metadata('idna')
datas += copy_metadata('urllib3')
datas += copy_metadata('scipy')
datas += copy_metadata('scikit-learn')
# datas += collect_data_files('Pillow')

a = Analysis(
    ['src/geniusai_server.py'],
    pathex=['src'],
    binaries=[],
    datas=datas,
    hiddenimports=['chromadb.telemetry.product.posthog', 'chromadb', 'chromadb.api.rust', 'torch', 'torchvision', 'PIL._imaging', 'scipy', 'scipy.special', 'sklearn', 'sklearn.utils._param_validation'],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=['tkinter', 'matplotlib', 'IPython', 'jupyter', 'notebook', 'pandas'],
    win_no_prefer_redirects=False,
    cipher=block_cipher,
    noarchive=False,
)
pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name='lrgenius-server',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=False,
    upx_exclude=[],
    name='lrgenius-server',
)
