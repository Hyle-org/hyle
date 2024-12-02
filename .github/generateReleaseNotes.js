const fs = require("fs");
const { execSync } = require("child_process");

const OUTPUT_FILE = "RELEASE_NOTES.md";

const GITMOJI_CATEGORIES = {
  "✨": { alias: ":sparkles:", category: "Features" },
  "🐛": { alias: ":bug:", category: "Bug Fixes" },
  "📝": { alias: ":books:", category: "Documentation" },
  "🔧": { alias: ":wrench:", category: "Configuration" },
  "✅": { alias: ":white_check_mark:", category: "Tests" },
  "⚡": { alias: ":zap:", category: "Performance" },
  "♻": { alias: ":recycle:", category: "Refactoring" },
  "🚀": { alias: ":rocket:", category: "Deployments" },
  "🚧": { alias: ":construction:", category: "Work in Progress" },
  "🔒": { alias: ":lock:", category: "Security" },
  "🌍": { alias: ":earth_africa:", category: "Localization" },
  "⬆️": { alias: ":arrow_up:", category: "Dependencies" },
  "⬇️": { alias: ":arrow_down:", category: "Dependencies" },
  "💥": { alias: ":boom:", category: "Breaking changes" },
};

function generateReleaseNotes() {
  const commits = execSync("git log --oneline $(git describe --tags --abbrev=0 @^)..@ --pretty=format:%s").toString().split("\n");

  const categoryCommits = {};
  Object.keys(GITMOJI_CATEGORIES).forEach((emoji) => {
    categoryCommits[emoji] = [];
  });
  let uncategorized = [];

  commits.forEach((commit) => {
    let categorized = false;
    for (const [emoji, { alias, category }] of Object.entries(GITMOJI_CATEGORIES)) {
      if (commit.startsWith(emoji) || commit.startsWith(alias)) {
        categoryCommits[emoji].push(commit.replace(alias, emoji).trim());
        categorized = true;
        break;
      }
    }
    if (!categorized) {
      uncategorized.push(commit);
    }
  });

  let releaseNotes = "";

  for (const [emoji, { category }] of Object.entries(GITMOJI_CATEGORIES)) {
    if (categoryCommits[emoji].length > 0) {
      releaseNotes += `## ${category}\n\n`;
      categoryCommits[emoji].forEach((commit) => {
        releaseNotes += `- ${commit}\n`;
      });
      releaseNotes += "\n";
    }
  }

  if (uncategorized.length > 0) {
    releaseNotes += "## Uncategorized\n\n";
    uncategorized.forEach((commit) => {
      releaseNotes += `- ${commit}\n`;
    });
    releaseNotes += "\n";
  }

  fs.writeFileSync(OUTPUT_FILE, releaseNotes);
  console.log(`Release notes generated in ${OUTPUT_FILE}`);
}

generateReleaseNotes();
