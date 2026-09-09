Given('利用者「山田」がログインする', () => loginAs('山田'));
Then('画面に「ようこそ 👋」と表示される', () => expectGreeting('ようこそ 👋'));
