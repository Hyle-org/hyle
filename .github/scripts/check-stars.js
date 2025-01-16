const fs = require('fs');
const axios = require('axios');

(async () => {
  const repo = process.env.GITHUB_REPOSITORY; // Nom du dépôt (ex: owner/repo)
  const slackWebhook = process.env.SLACK_WEBHOOK;

  // API GitHub pour récupérer les informations du dépôt
  const apiUrl = `https://api.github.com/repos/${repo}`;
  const cacheFile = 'stars_count.txt';

  try {
    // Obtenir les données actuelles du dépôt
    const response = await axios.get(apiUrl);
    const currentStars = response.data.stargazers_count;

    // Charger le nombre précédent d'étoiles
    let previousStars = 0;
    if (fs.existsSync(cacheFile)) {
      previousStars = parseInt(fs.readFileSync(cacheFile, 'utf8'), 10);
    }

    // Comparer et notifier si nécessaire
    if (currentStars !== previousStars) {
      console.log(`Le nombre d'étoiles a changé : ${previousStars} → ${currentStars}`);
      fs.writeFileSync(cacheFile, currentStars.toString(), 'utf8');

      // Envoyer une notification Slack
      if (slackWebhook) {
        await axios.post(slackWebhook, {
          text: `🚀 Le nombre d'étoiles a changé ! ${previousStars} → ${currentStars} ⭐`,
        });
        console.log('Notification envoyée à Slack.');
      } else {
        console.warn('Aucun webhook Slack fourni.');
      }
    } else {
      console.log('Pas de changement dans le nombre d\'étoiles.');
    }
  } catch (error) {
    console.error('Erreur:', error.message);
    process.exit(1);
  }
})();

