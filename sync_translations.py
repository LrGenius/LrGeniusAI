import os
import re

def extract_loc_keys(directory):
    # Matches LOC "$$$/LrGeniusAI/Module/Key=Default Value"
    # and LOC("$$$/LrGeniusAI/Module/Key=Default Value", ...)
    #
    # The string runs to the *matching* delimiter (backreference \1), not to
    # whichever quote character turns up first. The previous pattern used
    # [^"\']+, which stops dead at an apostrophe — so every default containing
    # one was silently truncated at it, and the truncation was then written to
    # all three translation files as if it were the real string. That is how
    # "Create 'Duplicates / Near Duplicates' collection" became "Create " in
    # shipped UI, along with a dozen others.
    #
    # (?:\\.|(?!\1).) consumes escaped characters as a unit so an escaped quote
    # inside the string does not end it either.
    pattern = re.compile(r'LOC\s*\(?\s*(["\'])(\$\$\$/LrGeniusAI/(?:\\.|(?!\1).)*)\1', re.DOTALL)

    keys = {}
    for root, _, files in os.walk(directory):
        for file in files:
            if file.endswith('.lua'):
                path = os.path.join(root, file)
                with open(path, 'r', encoding='utf-8', errors='ignore') as f:
                    content = f.read()
                    # group(2) is the key=value body; group(1) is the delimiter.
                    matches = [m[1] for m in pattern.findall(content)]
                    for m in matches:
                        if '=' in m:
                            key, val = m.split('=', 1)
                            keys[key] = val
                        else:
                            if m not in keys:
                                keys[m] = ""
    return keys

def load_translated_strings(path):
    strings = {}
    if not os.path.exists(path):
        return strings
    try:
        with open(path, 'r', encoding='utf-8') as f:
            lines = f.readlines()
    except UnicodeDecodeError:
        with open(path, 'r', encoding='utf-16') as f:
            lines = f.readlines()
            
    for line in lines:
        line = line.strip()
        if not line or line.startswith('#') or line.startswith('--'):
            continue
        match = re.search(r'["\'](\$\$\$/[^"\']+)["\']\s*=\s*"(.*)"\s*$', line)
        if match:
            # Undo the escaping the writer applies. Without this the round trip
            # is not idempotent: the writer turns " into \", the reader kept the
            # backslash, and the next run escaped that quote again. One string
            # in this repository had accumulated sixteen backslashes.
            strings[match.group(1)] = match.group(2).replace('\\"', '"')
    return strings

def sync_translations(lua_dir, target_path, base_strings=None):
    extracted_keys = extract_loc_keys(lua_dir)
    existing_strings = load_translated_strings(target_path)
    
    # If base_strings is provided (e.g. for DE/FR), we use it as the source of truth for keys
    if base_strings:
        keys_to_use = sorted(list(base_strings.keys()))
    else:
        # For EN, we use union of extracted and existing
        keys_to_use = sorted(list(set(extracted_keys.keys()) | set(existing_strings.keys())))
    
    new_content = []
    for key in keys_to_use:
        # Ignore old keys from previous project names if they aren't in current extraction
        if not key.startswith('$$$/LrGeniusAI/'):
            if key not in extracted_keys and (not base_strings or key not in base_strings):
                continue

        val = existing_strings.get(key)
        
        # For EN the Lua source *is* the string — that is what
        # LOC("$$$/Key=Default") means — so the extracted default wins whenever
        # there is one, and the existing file only fills in keys Lua no longer
        # mentions. Preferring the file instead, as this did, meant a corrected
        # default in Lua could never reach the shipped translations: the
        # truncated values above would have survived this very fix.
        if not base_strings:
            extracted = extracted_keys.get(key)
            if extracted:
                val = extracted
            elif not val:
                val = ""
        else:
            # For non-EN, if missing, use EN value as placeholder (or empty)
            if not val or val == "":
                # If the DE/FR value was missing, we can see if it was in EN
                val = base_strings.get(key) or ""
        
        val = val.replace('"', '\\"')
        new_content.append(f'"{key}" = "{val}"')
    
    output = '\n'.join(new_content)
    # UTF-16 LE with a BOM, matching what Lightroom reads and what these files
    # have always been committed as. The writer used to emit UTF-8 while the
    # reader below correctly tried UTF-16 first, so running this script — the
    # command CLAUDE.md and the edit hook both tell you to run — silently
    # rewrote all three files into an encoding Lightroom cannot read.
    with open(target_path, 'w', encoding='utf-16') as f:
        f.write(output)
    return load_translated_strings(target_path)

if __name__ == "__main__":
    # Resolved from this file's location rather than hardcoded, so the script
    # runs on any checkout. It previously pointed at an absolute path on one
    # developer's machine, which made it fail everywhere else — including CI
    # and the edit hook that tells you to run it.
    repo_root = os.path.dirname(os.path.abspath(__file__))
    lua_dir = os.path.join(repo_root, "plugin", "LrGeniusAI.lrdevplugin")
    trans_en = os.path.join(lua_dir, "TranslatedStrings_en.txt")
    trans_de = os.path.join(lua_dir, "TranslatedStrings_de.txt")
    trans_fr = os.path.join(lua_dir, "TranslatedStrings_fr.txt")


    en_strings = sync_translations(lua_dir, trans_en)
    print("Synched English.")
    sync_translations(lua_dir, trans_de, en_strings)
    print("Synched German.")
    sync_translations(lua_dir, trans_fr, en_strings)
    print("Synched French.")
