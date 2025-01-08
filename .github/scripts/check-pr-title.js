const prTitle = process.env.PR_TITLE;

if (!prTitle) {
  console.error("Error: PR title is not available.");
  process.exit(1);
}

// Regex to match gitmoji (:emoji:) or native emoji
const gitmojiOrEmojiRegex = /^(:[a-z0-9_+-]+:|[\u{1F300}-\u{1F5FF}\u{1F600}-\u{1F64F}\u{1F680}-\u{1F6FF}\u{1F700}-\u{1F77F}\u{1F780}-\u{1F7FF}\u{1F800}-\u{1F8FF}\u{1F900}-\u{1F9FF}\u{1FA00}-\u{1FA6F}\u{1FA70}-\u{1FAFF}\u{2600}-\u{26FF}\u{2700}-\u{27BF}])/u;

if (!gitmojiOrEmojiRegex.test(prTitle)) {
  console.error("Error: The PR title must start with a gitmoji (e.g., :sparkles:) or an emoji.");
  process.exit(1);
}

console.log("PR title is valid.");

