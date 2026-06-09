"""Ingest English Wiktionary data (wiktextract / kaikki raw dump).

Adds real English headwords (incl. multi-word phrases) and annotates POS, sense
counts, etymology source languages, and proper/form-of/offensive flags.

Design verified against the dataset (see workflow spec):
- English entries: lang_code == 'en'; skip redirects, '*'-reconstructions,
  romanizations.
- A headword is split across multiple JSON objects (one per POS and per
  etymology); MUST aggregate by lowercased word, summing senses and unioning
  pos/etymology.
- Etymology source language = args['2'] of a relation template (args['1'] is
  always 'en'); only {bor,inh,der,...} describe the headword (NOT m/l/cog).

Shape fields (length/scrabble/...) are left to a follow-up ingest/derive.py run.
"""
import gzip
import json

from ingest.db import connect, log_ingest
from ingest.paths import WIKTEXTRACT

# etymology template name -> relation label stored in word_etymology.relation
REL = {
    "bor": "bor", "bor+": "bor", "inh": "inh", "inh+": "inh",
    "der": "der", "der+": "der", "uder": "der",
    "lbor": "lbor", "slbor": "slbor", "calque": "calque",
}
# Wiktionary etymology-only / abbreviated codes -> canonical parent code
ETY_ONLY = {
    "LL.": "la", "ML.": "la", "NL.": "la", "EL.": "la", "VL.": "la",
    "la-med": "la", "la-ecc": "la", "la-cla": "la", "la-vul": "la", "la-new": "la",
    "fa-cls": "fa", "grc-koi": "grc",
}
OFFENSIVE = {"vulgar", "offensive", "derogatory", "pejorative", "ethnic-slur", "slur"}
FORMOF_TAGS = {"form-of", "alt-of", "alternative"}
SOURCE = "wiktextract"


def extract_ety(obj) -> list[tuple[str, str]]:
    rows = []
    for t in obj.get("etymology_templates", []) or []:
        rel = REL.get(t.get("name"))
        if not rel:
            continue
        code = (t.get("args") or {}).get("2")
        if not code:
            continue
        rows.append((ETY_ONLY.get(code, code), rel))
    return rows


def build(path) -> tuple[dict, int]:
    agg: dict = {}
    n_en = 0
    with gzip.open(path, "rt", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            if "redirect" in obj:
                continue
            w = obj.get("word")
            if not w or obj.get("lang_code") != "en":
                continue
            if w.startswith("*") or obj.get("pos") == "romanization":
                continue
            n_en += 1
            a = agg.get(w_low := w.lower())
            if a is None:
                a = agg[w_low] = {"n_senses": 0, "pos": set(), "ety": [], "primary": None,
                                  "proper": False, "offensive": False, "form_of": False,
                                  "ety_text": None, "gloss": None}
            senses = obj.get("senses") or []
            a["n_senses"] += len(senses)
            pos = obj.get("pos")
            if pos:
                a["pos"].add(pos)
                if pos == "name":
                    a["proper"] = True
            for code, rel in extract_ety(obj):
                a["ety"].append((code, rel))
                if a["primary"] is None:
                    a["primary"] = code
            if a["ety_text"] is None and obj.get("etymology_text"):
                a["ety_text"] = obj["etymology_text"]
            for s in senses:
                tagset = set(s.get("tags") or [])
                if "form_of" in s or "alt_of" in s or (tagset & FORMOF_TAGS):
                    a["form_of"] = True
                if tagset & OFFENSIVE:
                    a["offensive"] = True
                if a["gloss"] is None:
                    gl = s.get("glosses") or s.get("raw_glosses")
                    if gl:
                        a["gloss"] = gl[0]
    return agg, n_en


def main() -> None:
    con = connect()
    cur = con.cursor()
    con.execute("PRAGMA synchronous = OFF")

    print(f"streaming {WIKTEXTRACT.name} ...", flush=True)
    agg, n_en = build(WIKTEXTRACT)
    print(f"parsed {n_en:,} English objects -> {len(agg):,} headwords; writing...", flush=True)

    # 1. ensure every headword exists
    existing = {w for (w,) in cur.execute("SELECT word FROM words")}
    cur.executemany(
        "INSERT OR IGNORE INTO words(word, in_wiktionary) VALUES (?, 1)",
        [(k,) for k in agg if k not in existing],
    )
    con.commit()

    # 2. id map for all headwords (full reload includes the rows just inserted)
    idmap = {w: i for (i, w) in cur.execute("SELECT id, word FROM words")}

    # 3. annotate lexical fields + flags; collect pos / etymology rows
    upd, pos_rows, ety_rows = [], [], []
    for k, a in agg.items():
        wid = idmap[k]
        upd.append((a["n_senses"], a["primary"], a["ety_text"], a["gloss"],
                    int(a["proper"]), int(a["form_of"]), int(a["offensive"]), wid))
        pos_rows.extend((wid, p, SOURCE) for p in a["pos"])
        ety_rows.extend((wid, lang, rel, SOURCE) for (lang, rel) in set(a["ety"]))

    cur.executemany(
        "UPDATE words SET in_wiktionary=1, n_senses=?, etymology_lang=?, etymology_text=?, "
        "gloss=?, is_proper=?, is_form_of=?, is_offensive=? WHERE id=?",
        upd,
    )
    cur.executemany("INSERT OR IGNORE INTO word_pos(word_id, pos, source) VALUES (?, ?, ?)", pos_rows)
    cur.executemany(
        "INSERT OR IGNORE INTO word_etymology(word_id, lang, relation, source) VALUES (?, ?, ?, ?)",
        ety_rows,
    )
    con.commit()

    added = len(agg) - sum(1 for k in agg if k in existing)
    log_ingest(con, "wiktextract", f"{n_en} en objs, {len(agg)} headwords, {added} new", len(agg))
    print(f"wiktextract: {len(agg):,} headwords ({added:,} new), "
          f"{len(pos_rows):,} pos rows, {len(ety_rows):,} etymology rows", flush=True)
    con.close()


if __name__ == "__main__":
    main()
