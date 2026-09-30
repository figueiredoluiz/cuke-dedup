Given('利用者「山田」がログインする') { login_as('山田') }
Then('画面に「ようこそ 👋」と表示される') { expect_greeting('ようこそ 👋') }
