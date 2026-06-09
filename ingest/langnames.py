"""Populate lang_names: etymology language code -> full English name.

Covers every code appearing on >= 3 words (~99.3% of word-instances); the long
tail falls back to the raw code in the UI. Codes are Wiktionary / ISO 639.
"""
from ingest.db import connect, log_ingest

LANG = {
    "la": "Latin", "enm": "Middle English", "fr": "French", "grc": "Ancient Greek",
    "de": "German", "it": "Italian", "es": "Spanish", "ang": "Old English",
    "ja": "Japanese", "frm": "Middle French", "cmn": "Mandarin Chinese", "ar": "Arabic",
    "ga": "Irish", "ru": "Russian", "fro": "Old French", "pl": "Polish",
    "cmn-pinyin": "Mandarin Chinese (Pinyin)", "nl": "Dutch", "sa": "Sanskrit", "hi": "Hindi",
    "he": "Hebrew", "uk": "Ukrainian", "cy": "Welsh", "pt": "Portuguese", "mul": "Translingual",
    "non": "Old Norse", "gd": "Scottish Gaelic", "hy": "Armenian", "fa": "Persian", "mi": "Maori",
    "sv": "Swedish", "af": "Afrikaans", "cs": "Czech", "el": "Greek", "yi": "Yiddish",
    "ko": "Korean", "xno": "Anglo-Norman", "yue": "Cantonese", "mni": "Meitei", "zh": "Chinese",
    "sco": "Scots", "ms": "Malay", "no": "Norwegian", "haw": "Hawaiian", "tr": "Turkish",
    "tl": "Tagalog", "vi": "Vietnamese", "da": "Danish", "hu": "Hungarian", "sh": "Serbo-Croatian",
    "th": "Thai", "bn": "Bengali", "inc-hnd": "Hindustani", "ta": "Tamil", "ka": "Georgian",
    "ur": "Urdu", "nan-hok": "Hokkien", "ota": "Ottoman Turkish", "cmn-wadegile": "Mandarin Chinese (Wade-Giles)",
    "ug": "Uyghur", "pa": "Punjabi", "my": "Burmese", "ca": "Catalan", "nrf": "Norman",
    "ro": "Romanian", "fi": "Finnish", "sw": "Swahili", "bo": "Tibetan", "yo": "Yoruba",
    "dum": "Middle Dutch", "hbo": "Biblical Hebrew", "kw": "Cornish", "bg": "Bulgarian",
    "id": "Indonesian", "egy": "Egyptian", "te": "Telugu", "be": "Belarusian", "mr": "Marathi",
    "ceb": "Cebuano", "cmn-tongyong": "Mandarin Chinese (Tongyong Pinyin)", "am": "Amharic",
    "ml": "Malayalam", "oj": "Ojibwe", "fa-ira": "Iranian Persian", "gmq": "North Germanic",
    "km": "Khmer", "gl": "Galician", "gkm": "Medieval Greek", "sla": "Slavic", "zu": "Zulu",
    "gu": "Gujarati", "ne": "Nepali", "sq": "Albanian", "is": "Icelandic", "mn": "Mongolian",
    "sk": "Slovak", "nan": "Min Nan Chinese", "gem-pro": "Proto-Germanic", "iu": "Inuktitut",
    "nci": "Classical Nahuatl", "mk": "Macedonian", "alg": "Algonquian", "cr": "Cree",
    "sl": "Slovene", "kn": "Kannada", "tn": "Tswana", "yxg": "Yagara", "es-MX": "Mexican Spanish",
    "gem": "Germanic", "hop": "Hopi", "eu": "Basque", "si": "Sinhalese", "az": "Azerbaijani",
    "ig": "Igbo", "pt-BR": "Brazilian Portuguese", "lt": "Lithuanian", "wam": "Massachusett",
    "gml": "Middle Low German", "xpi": "Pictish", "cel": "Celtic", "mg": "Malagasy", "rom": "Romani",
    "xdk": "Dharug", "ak": "Akan", "as": "Assamese", "gsw": "Alemannic German", "kk": "Kazakh",
    "nah": "Nahuatl", "ha": "Hausa", "jam": "Jamaican Creole", "pam": "Kapampangan",
    "fr-CA": "Canadian French", "fy": "West Frisian", "nds": "Low German", "qu": "Quechua",
    "roa": "Romance", "sla-pro": "Proto-Slavic", "xcb": "Cumbric", "xh": "Xhosa", "ht": "Haitian Creole",
    "sga": "Old Irish", "akk": "Akkadian", "cel-bry": "Brythonic", "chn": "Chinook Jargon",
    "gv": "Manx", "lo": "Lao", "ps": "Pashto", "jv": "Javanese", "ky": "Kyrgyz", "sux": "Sumerian",
    "trk": "Turkic", "xnt": "Narragansett", "arc": "Aramaic", "ilo": "Ilocano",
    "ine-pro": "Proto-Indo-European", "tup": "Tupi", "arz": "Egyptian Arabic", "mt": "Maltese",
    "mus": "Muscogee", "pim": "Powhatan", "scn": "Sicilian", "sm": "Samoan", "en": "English",
    "kld": "Gamilaraay", "pdc": "Pennsylvania German", "tpw": "Old Tupi", "fo": "Faroese",
    "dz": "Dzongkha", "uz": "Uzbek", "arn": "Mapuche", "et": "Estonian", "mic": "Mi'kmaq",
    "nys": "Nyungar", "pi": "Pali", "wrh": "Wiradhuri", "dv": "Maldivian", "jbo": "Lojban",
    "kl": "Greenlandic", "lv": "Latvian", "ary": "Moroccan Arabic", "frc": "Cajun French",
    "nap": "Neapolitan", "nv": "Navajo", "rme": "Angloromani", "tnq": "Taino", "to": "Tongan",
    "unm": "Unami", "wo": "Wolof", "abe": "Abenaki", "bnt": "Bantu", "br": "Breton",
    "cel-bry-pro": "Proto-Brythonic", "del": "Lenape", "mga": "Middle Irish", "oc": "Occitan",
    "sn": "Shona", "so": "Somali", "st": "Sotho", "ban": "Balinese", "enm-nor": "Northern Middle English",
    "kg": "Kongo", "mnc": "Manchu", "naq": "Khoekhoe", "tg": "Tajik", "zhx-teo": "Teochew",
    "ae": "Avestan", "aus-pam": "Pama-Nyungan", "ch": "Chamorro", "cho": "Choctaw",
    "cpi": "Chinese Pidgin English", "ff": "Fula", "gn": "Guarani", "la-lat": "Late Latin",
    "lg": "Ganda", "lkt": "Lakota", "mnk": "Mandinka", "ny": "Chichewa", "pal": "Middle Persian",
    "sai-car": "Cariban", "trk-oat": "Old Anatolian Turkish", "ty": "Tahitian", "cel-gae": "Goidelic",
    "dak": "Dakota", "eo": "Esperanto", "gmh": "Middle High German", "gmw-pro": "Proto-West Germanic",
    "ik": "Inupiaq", "mas": "Maasai", "nb": "Norwegian Bokmal", "see": "Seneca", "shh": "Shoshone",
    "syc": "Classical Syriac", "tli": "Tlingit", "wya": "Wyandot", "alg-eas": "Eastern Algonquian",
    "gez": "Ge'ez", "iro": "Iroquoian", "moh": "Mohawk", "nrn": "Norn", "owl": "Old Welsh",
    "pld": "Polari", "rw": "Kinyarwanda", "sd": "Sindhi", "tk": "Turkmen", "tpi": "Tok Pisin",
    "vec": "Venetian", "apc": "Levantine Arabic", "cdo": "Min Dong Chinese", "chr": "Cherokee",
    "cop": "Coptic", "fro-nor": "Norman Old French", "goh": "Old High German", "hit": "Hittite",
    "kok": "Konkani", "ks": "Kashmiri", "lut": "Lushootseed", "myn": "Mayan", "nn": "Norwegian Nynorsk",
    "pml": "Lingua Franca (Mediterranean)", "sco-smi": "Middle Scots", "sem": "Semitic",
    "sem-pro": "Proto-Semitic", "zhx-tai": "Taishanese", "aaq": "Eastern Abenaki", "alq": "Algonquin",
    "bm": "Bambara", "car": "Galibi Carib", "cel-pro": "Proto-Celtic", "cro": "Crow",
    "esu": "Central Alaskan Yup'ik", "got": "Gothic", "gyn": "Guyanese Creole", "hur": "Hurrian",
    "ki": "Kikuyu", "kmr": "Northern Kurdish", "mh": "Marshallese", "moe": "Innu", "or": "Odia",
    "pag": "Pangasinan", "peo": "Old Persian", "qya": "Quenya", "rap": "Rapa Nui", "rm": "Romansch",
    "sjw": "Shawnee", "ti": "Tigrinya", "umu": "Munsee", "wlm": "Middle Welsh", "xcl": "Old Armenian",
    "yua": "Yucatec Maya", "arw": "Arawak", "ast": "Asturian", "bla": "Blackfoot", "chc": "Catawba",
    "dsb": "Lower Sorbian", "fj": "Fijian", "fon": "Fon", "gaa": "Ga", "gul": "Gullah",
    "ium": "Iu Mien", "la-ren": "Renaissance Latin", "lou": "Louisiana Creole", "mia": "Miami",
    "nds-de": "German Low German", "oka": "Okanagan", "ood": "O'odham", "pqm": "Maliseet-Passamaquoddy",
    "ryu": "Okinawan", "ute": "Ute", "wth": "Wathawurrung", "xng": "Middle Mongol", "xpq": "Mohegan-Pequot",
    "yur": "Yurok", "zhx": "Chinese (other varieties)", "RL.": "Latin (proper noun derived)",
    "aer": "Eastern Arrernte", "aii": "Assyrian Neo-Aramaic", "bi": "Bislama", "chh": "Chinook",
    "crh": "Crimean Tatar", "cu": "Old Church Slavonic", "din": "Dinka", "ee": "Ewe",
    "es-PR": "Puerto Rican Spanish", "frk": "Frankish", "gcf": "Guadeloupean Creole", "gil": "Gilbertese",
    "hil": "Hiligaynon", "ibb": "Ibibio", "ike": "Eastern Canadian Inuktitut", "kla": "Klamath-Modoc",
    "mww": "Hmong Daw", "orv": "Old East Slavic", "pdt": "Plautdietsch", "pjt": "Pitjantjatjara",
    "rue": "Rusyn", "se": "Northern Sami", "shs": "Shuswap", "sjn": "Sindarin", "su": "Sundanese",
    "tyv": "Tuvan", "vls": "West Flemish", "was": "Washo", "wuu-sha": "Shanghainese",
    "xww": "Wemba-Wemba", "afb": "Gulf Arabic", "arq": "Algerian Arabic", "art-vlh": "High Valyrian",
    "atj": "Atikamekw", "bar": "Bavarian", "bcl": "Central Bikol", "ber": "Berber", "cel-gau": "Gaulish",
    "cmg": "Classical Mongolian", "co": "Corsican", "crg": "Michif", "crk": "Plains Cree",
    "csi": "Coast Miwok", "dgr": "Dogrib", "dra": "Dravidian", "gmq-oda": "Old Danish",
    "hak": "Hakka Chinese", "kaw": "Old Javanese", "kln": "Kalenjin", "kls": "Kalasha",
    "kmb": "Kimbundu", "ksd": "Kuanua", "lb": "Luxembourgish", "lmo": "Lombard", "ln": "Lingala",
    "ltc": "Middle Chinese", "lzh": "Literary Chinese", "nds-nl": "Dutch Low Saxon", "ng": "Ndonga",
    "nuk": "Nuu-chah-nulth", "obt": "Old Breton", "one": "Oneida", "otk": "Old Turkic",
    "phr": "Pahari-Potwari", "pot": "Potawatomi", "poz-pol": "Polynesian", "pro": "Old Occitan",
    "prv": "Provencal", "pua": "Western Highland Purepecha", "sah": "Yakut", "sco-osc": "Old Scots",
    "sio": "Siouan", "srn": "Sranan Tongo", "ss": "Swazi", "szl": "Silesian", "tkl": "Tokelauan",
    "ts": "Tsonga", "tt": "Tatar", "tvl": "Tuvaluan", "war": "Waray-Waray", "wnw": "Wintu",
    "xul": "Ngunawal", "xuu": "Khwe", "zlw-opl": "Old Polish",
}


def main() -> None:
    con = connect()
    con.execute("DELETE FROM lang_names")
    con.executemany("INSERT INTO lang_names(code, name) VALUES (?, ?)", LANG.items())
    con.commit()
    log_ingest(con, "langnames", "etymology code -> name", len(LANG))
    print(f"lang_names: {len(LANG)} codes mapped")
    con.close()


if __name__ == "__main__":
    main()
