Given('the account is enabled') { verify_account() }
Then('the account is not enabled') { verify_account() }
Given('the parcel is ready') { parcel.verify('ready') }
Then('the parcel is now ready') { parcel.verify('ready') }
