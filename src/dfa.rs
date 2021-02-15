use serde_derive::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule<'a> {
    Allow(&'a str),
    Disallow(&'a str),
}

impl<'a> Rule<'a> {
    fn inner(&self) -> &str {
        match self {
            Rule::Allow(inner) => inner,
            Rule::Disallow(inner) => inner,
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
enum Edge {
    MatchChar(char),
    MatchAny,
    MatchEow,
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
struct Transition(Edge, usize);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum State {
    Allow,
    Disallow,
    Intermediate,
}

/// A Cylon is a DFA that recognizes rules from a compiled robots.txt
/// file. By providing it a URL path, it can decide whether or not
/// the robots file that compiled it allows or disallows that path in
/// roughly O(n) time, where n is the length of the path.
#[derive(Debug, Serialize, Deserialize)]
pub struct Cylon {
    states: Vec<State>,
    transitions: Vec<Vec<Transition>>,
}

impl Cylon {
    /// Match whether the rules allow or disallow the target path.
    pub fn allow(&self, path: &str) -> bool {
        let mut state = path.chars().fold(2, |state, path_char| {
            let t = &self.transitions[state];
            t.iter()
                .rev()
                // Pick the last transition to always prioritize MatchChar
                // over MatchAny (which will always be the first transition.)
                .find(|transition| match transition {
                    Transition(Edge::MatchAny, ..) => true,
                    Transition(Edge::MatchEow, ..) => false,
                    Transition(Edge::MatchChar(edge_char), ..) => *edge_char == path_char,
                })
                .map(|Transition(.., next_state)| *next_state)
                // We are guaranteed at least one matching state because of
                // the way the DFA is constructed.
                .unwrap()
        });

        // Follow the EoW transition, if necessary
        let t = &self.transitions[state];
        state = t
            .iter()
            .rev()
            .find(|transition| match transition {
                Transition(Edge::MatchEow, ..) => true,
                Transition(Edge::MatchAny, ..) => true,
                _ => false,
            })
            .map(|Transition(.., next_state)| *next_state)
            .unwrap_or(state);

        match self.states[state] {
            State::Allow => true,
            State::Disallow => false,
            // Intermediate states are not preserved in the DFA
            State::Intermediate => unreachable!(),
        }
    }

    /// Compile a machine from a list of rules.
    pub fn compile(mut rules: Vec<Rule>) -> Self {
        // This algorithm constructs a DFA by doing BFS over the prefix tree of
        // paths in the provided list of rules. However, for performance reasons
        // it does not actually build a tree structure. (Vecs have better
        // cache-locality by avoiding random memory access.)

        let mut transitions: Vec<Vec<Transition>> = vec![
            vec![Transition(Edge::MatchAny, 0)],
            vec![Transition(Edge::MatchAny, 1)],
        ];
        let mut states: Vec<State> = vec![State::Allow, State::Disallow];

        rules.sort_by(|a, b| Ord::cmp(a.inner(), b.inner()));

        let mut queue = vec![("", 0, 0, State::Intermediate)];
        while !queue.is_empty() {
            // parent_prefix is the "parent node" in the prefix tree. We are
            // going to visit its children by filtering from the list of
            // paths only the paths that start with the parent_prefix.
            // wildcard_state is a node to jump to when an unmatched character
            // is encountered. This is usually a node higher up in the tree
            // that can match any character legally, but is also a prefix
            // (read: ancestor) of the current node.
            let (parent_prefix, mut wildcard_state, parent_state, state) = queue.remove(0);
            let last_char = parent_prefix.chars().last();

            wildcard_state = match state {
                State::Allow => 0,
                State::Disallow if last_char == Some('$') => wildcard_state,
                State::Disallow => 1,
                State::Intermediate => wildcard_state,
            };

            let mut t = match last_char {
                Some('$') => {
                    // The EOW character cannot match anything else
                    vec![Transition(Edge::MatchAny, wildcard_state)]
                }
                Some('*') => {
                    // The wildcard character overrides the wildcard state
                    vec![Transition(Edge::MatchAny, transitions.len())]
                }
                _ => {
                    // Every other state has a self-loop that matches anything
                    vec![Transition(Edge::MatchAny, wildcard_state)]
                }
            };

            let mut curr_prefix = "";
            rules
                .iter()
                .map(Rule::inner)
                .zip(&rules)
                .filter(|(path, _)| (*path).starts_with(parent_prefix))
                .filter(|(path, _)| (*path) != parent_prefix)
                .for_each(|(path, rule)| {
                    let child_prefix = &path[0..parent_prefix.len() + 1];
                    if curr_prefix == child_prefix {
                        // We only want to visit a child node once, but
                        // many rules might have the same child_prefix, so
                        // we skip the duplicates after the first time
                        // we see a prefix. (This could be a filter(), but
                        // it's a bit hard to encode earlier in the chain.)
                        return;
                    }
                    curr_prefix = child_prefix;

                    let eow = child_prefix == path;
                    let state = match (rule, eow) {
                        (Rule::Allow(..), true) => State::Allow,
                        (Rule::Disallow(..), true) => State::Disallow,
                        _ => State::Intermediate,
                    };

                    queue.push((child_prefix, wildcard_state, transitions.len(), state));

                    // NB: we can predict what state index the child
                    // will have before it's even pushed onto the state vec.
                    let child_index = transitions.len() + queue.len();
                    let edge_char = child_prefix.chars().last().unwrap();
                    let transition = Transition(
                        match edge_char {
                            '*' => Edge::MatchAny,
                            '$' => Edge::MatchEow,
                            c => Edge::MatchChar(c),
                        },
                        child_index,
                    );

                    // Add transitions from the parent state to the child state
                    // so that the wildcard character matches are optional.
                    if last_char == Some('*') {
                        let parent_t = &mut transitions[parent_state];
                        parent_t.push(transition);
                    }

                    t.push(transition);
                });

            states.push(match state {
                State::Allow | State::Disallow => state,
                State::Intermediate => states[wildcard_state],
            });
            transitions.push(t);
        }

        Self {
            states,
            transitions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! t {
        ('*' => $x:expr) => {
            Transition(Edge::MatchAny, $x)
        };
        ('$' => $x:expr) => {
            Transition(Edge::MatchEow, $x)
        };
        ($x:expr => $y:expr) => {
            Transition(Edge::MatchChar($x), $y)
        };
    }

    #[test]
    fn test_compile() {
        let rules = vec![
            Rule::Disallow("/"),
            Rule::Allow("/a"),
            Rule::Allow("/abc"),
            Rule::Allow("/b"),
        ];

        let expect_transitions = vec![
            vec![t!('*' => 0)],
            vec![t!('*' => 1)],
            vec![t!('*' => 0), t!('/' => 3)],               // ""
            vec![t!('*' => 1), t!('a' => 4), t!('b' => 5)], // "/"
            vec![t!('*' => 0), t!('b' => 6)],               // "/a"
            vec![t!('*' => 0)],                             // "/b"
            vec![t!('*' => 0), t!('c' => 7)],               // "/ab"
            vec![t!('*' => 0)],                             // "/abc"
        ];

        let expect_states = vec![
            State::Allow,
            State::Disallow,
            State::Allow,
            State::Disallow,
            State::Allow,
            State::Allow,
            State::Allow,
            State::Allow,
        ];

        let actual = Cylon::compile(rules);
        assert_eq!(actual.transitions, expect_transitions);
        assert_eq!(actual.states, expect_states);
    }

    #[test]
    fn test_compile_with_wildcard() {
        let rules = vec![Rule::Disallow("/"), Rule::Allow("/a"), Rule::Allow("/*.b")];

        let expect_transitions = vec![
            vec![t!('*' => 0)],
            vec![t!('*' => 1)],
            vec![t!('*' => 0), t!('/' => 3)], // ""
            vec![t!('*' => 1), t!('*' => 4), t!('a' => 5), t!('.' => 6)], // "/"
            vec![t!('*' => 4), t!('.' => 6)], // "/*"
            vec![t!('*' => 0)],               // "/a"
            vec![t!('*' => 1), t!('b' => 7)], // "/*."
            vec![t!('*' => 0)],               // "/*.b"
        ];

        let expect_states = vec![
            State::Allow,
            State::Disallow,
            State::Allow,
            State::Disallow,
            State::Disallow,
            State::Allow,
            State::Disallow,
            State::Allow,
        ];

        let actual = Cylon::compile(rules);
        assert_eq!(actual.transitions, expect_transitions);
        assert_eq!(actual.states, expect_states);
    }

    #[test]
    fn test_compile_tricky_wildcard() {
        let rules = vec![Rule::Disallow("/"), Rule::Allow("/*.")];

        let expect_transitions = vec![
            vec![t!('*' => 0)],
            vec![t!('*' => 1)],
            vec![t!('*' => 0), t!('/' => 3)],               // ""
            vec![t!('*' => 1), t!('*' => 4), t!('.' => 5)], // "/"
            vec![t!('*' => 4), t!('.' => 5)],               // "/*"
            vec![t!('*' => 0)],                             // "/*."
        ];

        let expect_states = vec![
            State::Allow,
            State::Disallow,
            State::Allow,
            State::Disallow,
            State::Disallow,
            State::Allow,
        ];

        let actual = Cylon::compile(rules);
        assert_eq!(actual.transitions, expect_transitions);
        assert_eq!(actual.states, expect_states);
    }

    #[test]
    fn test_compile_with_eow() {
        let rules = vec![
            Rule::Allow("/"),
            Rule::Disallow("/a$"),
            // Note that this rule is nonsensical. It will compile, but
            // no guarantees are made as to how it's matched. Rules should
            // use url-encoded strings to escape $.
            Rule::Disallow("/x$y"),
        ];

        let expect_transitions = vec![
            vec![t!('*' => 0)],
            vec![t!('*' => 1)],
            vec![t!('*' => 0), t!('/' => 3)],               // ""
            vec![t!('*' => 0), t!('a' => 4), t!('x' => 5)], // "/"
            vec![t!('*' => 0), t!('$' => 6)],               // "/a"
            vec![t!('*' => 0), t!('$' => 7)],               // "/x"
            vec![t!('*' => 0)],                             // "/a$"
            vec![t!('*' => 0), t!('y' => 8)],               // "/x$"
            vec![t!('*' => 1)],                             // "/x$y"
        ];

        let expect_states = vec![
            State::Allow,
            State::Disallow,
            State::Allow,
            State::Allow,
            State::Allow,
            State::Allow,
            State::Disallow,
            State::Allow,
            State::Disallow,
        ];

        let actual = Cylon::compile(rules);
        assert_eq!(actual.transitions, expect_transitions);
        assert_eq!(actual.states, expect_states);
    }

    #[test]
    fn test_allow() {
        let rules = vec![
            Rule::Disallow("/"),
            Rule::Allow("/a"),
            Rule::Allow("/abc"),
            Rule::Allow("/b"),
        ];

        let machine = Cylon::compile(rules);
        assert_eq!(false, machine.allow("/"));
        assert_eq!(true, machine.allow("/a"));
        assert_eq!(true, machine.allow("/a/b"));
        assert_eq!(true, machine.allow("/a"));
        assert_eq!(true, machine.allow("/abc"));
        assert_eq!(true, machine.allow("/abc/def"));
        assert_eq!(true, machine.allow("/b"));
        assert_eq!(true, machine.allow("/b/c"));
    }

    #[test]
    fn test_allow_match_any() {
        let rules = vec![
            Rule::Allow("/"),
            Rule::Disallow("/secret/*.txt"),
            Rule::Disallow("/private/*"),
        ];

        let machine = Cylon::compile(rules);
        assert_eq!(true, machine.allow("/"));
        assert_eq!(true, machine.allow("/abc"));
        assert_eq!(false, machine.allow("/secret/abc.txt"));
        assert_eq!(false, machine.allow("/secret/123.txt"));
        assert_eq!(true, machine.allow("/secret/abc.csv"));
        assert_eq!(true, machine.allow("/secret/123.csv"));
        assert_eq!(false, machine.allow("/private/abc.txt"));
        assert_eq!(false, machine.allow("/private/123.txt"));
        assert_eq!(false, machine.allow("/private/abc.csv"));
        assert_eq!(false, machine.allow("/private/123.csv"));
    }

    #[test]
    fn test_allow_match_eow() {
        let rules = vec![
            Rule::Allow("/"),
            Rule::Disallow("/ignore$"),
            Rule::Disallow("/foo$bar"),
        ];

        let machine = Cylon::compile(rules);
        assert_eq!(true, machine.allow("/"));
        assert_eq!(true, machine.allow("/abc"));
        assert_eq!(false, machine.allow("/ignore"));
        assert_eq!(true, machine.allow("/ignoreabc"));
        assert_eq!(true, machine.allow("/ignore/abc"));
        // These are technically undefined, and no behavior
        // is guaranteed since the rule is malformed. However
        // it is safer to accept them rather than reject them.
        assert_eq!(true, machine.allow("/foo"));
        assert_eq!(true, machine.allow("/foo$bar"));
    }

    #[test]
    fn test_allow_more_complicated() {
        let rules = vec![
            Rule::Allow("/"),
            Rule::Disallow("/a$"),
            Rule::Disallow("/abc"),
            Rule::Allow("/abc/*"),
            Rule::Disallow("/foo/bar"),
            Rule::Allow("/*/bar"),
            Rule::Disallow("/www/*/images"),
            Rule::Allow("/www/public/images"),
        ];

        let machine = Cylon::compile(rules);
        assert_eq!(true, machine.allow("/"));
        assert_eq!(true, machine.allow("/directory"));
        assert_eq!(false, machine.allow("/a"));
        assert_eq!(true, machine.allow("/ab"));
        assert_eq!(false, machine.allow("/abc"));
        assert_eq!(true, machine.allow("/abc/123"));
        assert_eq!(true, machine.allow("/foo"));
        assert_eq!(true, machine.allow("/foobar"));
        assert_eq!(false, machine.allow("/foo/bar"));
        assert_eq!(false, machine.allow("/foo/bar/baz"));
        assert_eq!(true, machine.allow("/baz/bar"));
        assert_eq!(false, machine.allow("/www/cat/images"));
        assert_eq!(true, machine.allow("/www/public/images"));
    }

    #[test]
    fn test_matches() {
        // Test cases from:
        // https://developers.google.com/search/reference/robots_txt#group-member-rules

        let machine = Cylon::compile(vec![Rule::Disallow("/"), Rule::Allow("/fish")]);
        assert_eq!(true, machine.allow("/fish"));
        assert_eq!(true, machine.allow("/fish.html"));
        assert_eq!(true, machine.allow("/fish/salmon.html"));
        assert_eq!(true, machine.allow("/fishheads.html"));
        assert_eq!(true, machine.allow("/fishheads/yummy.html"));
        assert_eq!(true, machine.allow("/fish.php?id=anything"));
        assert_eq!(false, machine.allow("/Fish.asp"));
        assert_eq!(false, machine.allow("/catfish"));
        assert_eq!(false, machine.allow("/?id=fish"));

        let machine = Cylon::compile(vec![Rule::Disallow("/"), Rule::Allow("/fish*")]);
        assert_eq!(true, machine.allow("/fish"));
        assert_eq!(true, machine.allow("/fish.html"));
        assert_eq!(true, machine.allow("/fish/salmon.html"));
        assert_eq!(true, machine.allow("/fishheads.html"));
        assert_eq!(true, machine.allow("/fishheads/yummy.html"));
        assert_eq!(true, machine.allow("/fish.php?id=anything"));
        assert_eq!(false, machine.allow("/Fish.asp"));
        assert_eq!(false, machine.allow("/catfish"));
        assert_eq!(false, machine.allow("/?id=fish"));

        let machine = Cylon::compile(vec![Rule::Disallow("/"), Rule::Allow("/fish/")]);
        assert_eq!(true, machine.allow("/fish/"));
        assert_eq!(true, machine.allow("/fish/?id=anything"));
        assert_eq!(true, machine.allow("/fish/salmon.htm"));
        assert_eq!(false, machine.allow("/fish"));
        assert_eq!(false, machine.allow("/fish.html"));
        assert_eq!(false, machine.allow("/Fish/Salmon.asp"));

        let machine = Cylon::compile(vec![Rule::Disallow("/"), Rule::Allow("/*.php")]);
        assert_eq!(true, machine.allow("/filename.php"));
        assert_eq!(true, machine.allow("/folder/filename.php"));
        assert_eq!(true, machine.allow("/folder/filename.php?parameters"));
        assert_eq!(true, machine.allow("/folder/any.php.file.html"));
        assert_eq!(true, machine.allow("/filename.php/"));
        assert_eq!(false, machine.allow("/"));
        assert_eq!(false, machine.allow("/windows.PHP"));

        let machine = Cylon::compile(vec![Rule::Disallow("/"), Rule::Allow("/*.php$")]);
        assert_eq!(true, machine.allow("/filename.php"));
        assert_eq!(true, machine.allow("/folder/filename.php"));
        assert_eq!(false, machine.allow("/filename.php?parameters"));
        assert_eq!(false, machine.allow("/filename.php/"));
        assert_eq!(false, machine.allow("/filename.php5"));
        assert_eq!(false, machine.allow("/windows.PHP"));

        let machine = Cylon::compile(vec![Rule::Disallow("/"), Rule::Allow("/fish*.php")]);
        assert_eq!(true, machine.allow("/fish.php"));
        assert_eq!(true, machine.allow("/fishheads/catfish.php?parameters"));
        assert_eq!(false, machine.allow("/Fish.PHP"));
    }
}