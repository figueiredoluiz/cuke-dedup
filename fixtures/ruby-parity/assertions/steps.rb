Given('the exact assertion') { expect(exact_parcel).to eq('ready') }
Then('another exact assertion') { expect(exact_parcel).to eq('ready') }
Given('the parcel is verified') { expect(parcel).to eq('ready') }
Then('the parcel is now verified') { expect(parcel).not_to eq('ready') }
Given('the account is enabled') { expect(account).to be_enabled }
Then('the account is not enabled') { expect(account).not_to be_enabled }
