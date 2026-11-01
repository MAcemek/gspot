//! Stronger single-word replacements for "very <word>" phrases.

/// Maps a base word to stronger replacements for "very <word>", best first. Sorted by key.
static TABLE: &[(&str, &[&str])] = &[
    ("accurate", &["precise", "exact", "correct"]),
    ("afraid", &["terrified", "petrified", "frightened"]),
    ("angry", &["furious", "enraged", "livid"]),
    ("bad", &["terrible", "awful", "dreadful"]),
    ("badly", &["terribly", "awfully", "dreadfully"]),
    ("beautiful", &["gorgeous", "stunning", "exquisite"]),
    ("big", &["huge", "enormous", "massive"]),
    ("boring", &["tedious", "dull", "monotonous"]),
    ("brave", &["courageous", "fearless", "valiant"]),
    ("bright", &["brilliant", "dazzling", "radiant"]),
    ("busy", &["swamped", "overloaded", "hectic"]),
    ("calm", &["serene", "tranquil", "peaceful"]),
    ("careful", &["meticulous", "cautious", "painstaking"]),
    ("cheap", &["dirt-cheap", "economical"]),
    ("clean", &["spotless", "immaculate", "pristine"]),
    ("clear", &["crystal-clear", "unmistakable", "evident"]),
    ("cold", &["freezing", "frigid", "icy"]),
    ("common", &["widespread", "prevalent", "ubiquitous"]),
    ("complex", &["intricate", "complicated", "elaborate"]),
    ("confident", &["self-assured", "assured", "poised"]),
    ("crazy", &["insane", "absurd", "outrageous"]),
    ("creative", &["inventive", "imaginative", "innovative"]),
    ("dangerous", &["hazardous", "perilous", "treacherous"]),
    ("dark", &["pitch-black", "inky"]),
    ("deep", &["profound", "cavernous"]),
    ("delicious", &["scrumptious", "mouthwatering", "delectable"]),
    ("detailed", &["thorough", "comprehensive", "meticulous"]),
    ("different", &["distinct", "dissimilar"]),
    ("difficult", &["arduous", "grueling", "formidable"]),
    ("dirty", &["filthy", "grimy", "grubby"]),
    ("dry", &["parched", "arid", "bone-dry"]),
    ("easy", &["effortless", "painless", "simple"]),
    ("effective", &["potent", "powerful"]),
    ("empty", &["vacant", "barren", "deserted"]),
    ("excited", &["thrilled", "elated", "exhilarated"]),
    ("exciting", &["thrilling", "exhilarating", "electrifying"]),
    ("expensive", &["costly", "exorbitant"]),
    ("famous", &["renowned", "celebrated", "legendary"]),
    ("far", &["distant", "remote", "faraway"]),
    ("fast", &["rapid", "swift", "speedy"]),
    ("friendly", &["amiable", "affable", "welcoming"]),
    ("full", &["packed", "crammed"]),
    ("fun", &["enjoyable", "entertaining", "delightful"]),
    ("funny", &["hilarious", "comical", "amusing"]),
    ("glad", &["delighted", "thrilled", "overjoyed"]),
    ("good", &["excellent", "outstanding", "superb"]),
    ("good-looking", &["handsome", "gorgeous", "striking"]),
    ("grateful", &["thankful", "appreciative", "indebted"]),
    ("happy", &["delighted", "joyful", "ecstatic"]),
    ("hard", &["grueling", "arduous", "strenuous"]),
    ("healthy", &["robust", "vigorous", "thriving"]),
    ("heavy", &["hefty", "weighty", "massive"]),
    ("helpful", &["invaluable", "beneficial"]),
    ("high", &["towering", "lofty"]),
    ("honest", &["candid", "truthful", "forthright"]),
    ("hot", &["scorching", "sweltering", "boiling"]),
    ("hungry", &["starving", "famished", "ravenous"]),
    ("hurt", &["wounded", "injured"]),
    ("important", &["crucial", "essential", "vital"]),
    ("impressive", &["remarkable", "striking", "stunning"]),
    ("interesting", &["fascinating", "captivating", "intriguing"]),
    ("kind", &["generous", "compassionate", "caring"]),
    ("large", &["huge", "enormous", "massive"]),
    ("lazy", &["lethargic", "indolent", "slothful"]),
    ("long", &["lengthy", "extended", "endless"]),
    ("loud", &["deafening", "thunderous", "booming"]),
    ("lucky", &["fortunate", "blessed"]),
    ("mean", &["cruel", "nasty", "vicious"]),
    ("necessary", &["essential", "vital", "imperative"]),
    ("nervous", &["anxious", "jittery", "apprehensive"]),
    ("new", &["brand-new", "novel", "innovative"]),
    ("nice", &["lovely", "delightful", "wonderful"]),
    ("obvious", &["glaring", "blatant", "unmistakable"]),
    ("old", &["ancient", "aged"]),
    ("polite", &["courteous", "respectful", "gracious"]),
    ("poor", &["destitute", "impoverished", "penniless"]),
    ("popular", &["sought-after", "beloved"]),
    ("pretty", &["lovely", "gorgeous", "stunning"]),
    ("quick", &["rapid", "swift", "speedy"]),
    ("quickly", &["rapidly", "swiftly", "hastily"]),
    ("quiet", &["silent", "hushed", "muted"]),
    ("rare", &["scarce", "exceptional"]),
    ("rich", &["wealthy", "affluent", "prosperous"]),
    ("rough", &["coarse", "jagged", "rugged"]),
    ("rude", &["disrespectful", "insolent", "impertinent"]),
    ("sad", &["miserable", "heartbroken", "sorrowful"]),
    ("safe", &["secure", "protected"]),
    ("scared", &["terrified", "petrified", "frightened"]),
    ("serious", &["grave", "severe", "critical"]),
    ("sharp", &["razor-sharp", "keen"]),
    ("short", &["brief", "concise"]),
    ("similar", &["near-identical", "alike"]),
    ("simple", &["basic", "straightforward", "elementary"]),
    ("sleepy", &["drowsy", "groggy"]),
    ("slow", &["sluggish", "glacial", "plodding"]),
    ("small", &["tiny", "minuscule", "miniature"]),
    ("smart", &["brilliant", "ingenious", "astute"]),
    ("smooth", &["silky", "sleek"]),
    ("soft", &["velvety", "plush", "downy"]),
    ("special", &["exceptional", "extraordinary", "remarkable"]),
    ("strong", &["powerful", "robust", "formidable"]),
    ("stupid", &["idiotic", "foolish", "senseless"]),
    ("successful", &["thriving", "prosperous", "accomplished"]),
    ("sure", &["certain", "confident", "positive"]),
    ("surprised", &["astonished", "amazed", "stunned"]),
    ("tall", &["towering", "statuesque"]),
    ("tasty", &["delicious", "flavorful", "delectable"]),
    ("thick", &["dense", "bulky"]),
    ("thin", &["slender", "slim"]),
    ("thirsty", &["parched", "dehydrated"]),
    ("tired", &["exhausted", "drained", "weary"]),
    ("ugly", &["hideous", "grotesque", "repulsive"]),
    ("unusual", &["extraordinary", "peculiar", "bizarre"]),
    ("upset", &["distraught", "distressed", "anguished"]),
    ("useful", &["invaluable", "practical", "handy"]),
    ("valuable", &["precious", "priceless", "invaluable"]),
    ("weak", &["feeble", "frail"]),
    ("wet", &["soaked", "drenched", "sodden"]),
    ("wide", &["broad", "expansive"]),
    ("worried", &["anxious", "apprehensive", "alarmed"]),
];

/// Stronger words that replace "very <word>", best first, or None when there is no good swap.
pub fn stronger_than_very(word: &str) -> Option<&'static [&'static str]> {
    let key = word.to_lowercase();
    TABLE
        .binary_search_by(|(k, _)| (*k).cmp(key.as_str()))
        .ok()
        .map(|i| TABLE[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys must be strictly ascending (sorted and unique) for binary search.
    #[test]
    fn table_sorted_and_unique() {
        for pair in TABLE.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{} !< {}", pair[0].0, pair[1].0);
        }
    }

    /// Every option is a clean lower-case single word different from its key.
    #[test]
    fn options_are_well_formed() {
        for (key, opts) in TABLE {
            for o in *opts {
                assert!(!o.is_empty(), "empty option for {key}");
                assert_eq!(*o, o.to_lowercase(), "not lower-case: {o}");
                assert!(!o.contains(' '), "space in option: {o}");
                assert_ne!(o, key, "option equals key: {key}");
            }
        }
    }

    /// Every key has between 2 and 4 options.
    #[test]
    fn option_counts_in_range() {
        for (key, opts) in TABLE {
            assert!((2..=4).contains(&opts.len()), "{key} has {}", opts.len());
        }
    }

    /// Lookups are case-insensitive and miss cleanly.
    #[test]
    fn lookups() {
        assert!(stronger_than_very("Tired").unwrap().contains(&"exhausted"));
        assert!(stronger_than_very("big").is_some());
        assert!(stronger_than_very("xylophone").is_none());
    }
}
