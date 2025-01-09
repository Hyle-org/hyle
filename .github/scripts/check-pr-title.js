const prTitle = process.env.PR_TITLE;

if (!prTitle) {
  console.error("Error: PR title is not available.");
  process.exit(1);
}

// Regex to match gitmoji (:emoji:) or Unicode emojis including modifiers
const gitmojiOrEmojiRegex = /^(:[a-z0-9_+-]+:|[\p{Emoji_Presentation}\p{Emoji}\uFE0F])/u;

if (!gitmojiOrEmojiRegex.test(prTitle)) {
  console.error("Error: The PR title must start with a gitmoji (e.g., :sparkles:) or an emoji.");
  process.exit(1);
}

console.log("PR title is valid.");

