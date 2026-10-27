#import <Contacts/Contacts.h>
#import <Foundation/Foundation.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>

bool messages_contacts_available(void) {
    @autoreleasepool {
        return [CNContactStore authorizationStatusForEntityType:CNEntityTypeContacts] == CNAuthorizationStatusAuthorized;
    }
}

unsigned char *messages_contact_names(size_t *length) {
    *length = 0;
    @autoreleasepool {
        // Checking an existing grant must never become a consent prompt.
        if (!messages_contacts_available()) {
            return NULL;
        }
        @try {
            CNContactStore *store = [[CNContactStore alloc] init];
            NSArray *keys = @[
                [CNContactFormatter descriptorForRequiredKeysForStyle:CNContactFormatterStyleFullName],
                CNContactOrganizationNameKey,
                CNContactPhoneNumbersKey,
                CNContactEmailAddressesKey
            ];
            CNContactFetchRequest *request = [[CNContactFetchRequest alloc] initWithKeysToFetch:keys];
            NSMutableArray *names = [[NSMutableArray alloc] init];
            NSError *error = nil;
            BOOL success = [store enumerateContactsWithFetchRequest:request error:&error usingBlock:^(CNContact *contact, BOOL *stop) {
                NSString *name = [CNContactFormatter stringFromContact:contact style:CNContactFormatterStyleFullName];
                if (name.length == 0) {
                    name = contact.organizationName;
                }
                if (name.length == 0) {
                    return;
                }
                for (CNLabeledValue<CNPhoneNumber *> *phone in contact.phoneNumbers) {
                    [names addObject:@{@"handle":phone.value.stringValue, @"name":name}];
                }
                for (CNLabeledValue<NSString *> *email in contact.emailAddresses) {
                    [names addObject:@{@"handle":email.value, @"name":name}];
                }
            }];
            if (!success) {
                return NULL;
            }
            NSData *data = [NSJSONSerialization dataWithJSONObject:names options:0 error:&error];
            if (data == nil || data.length > 16 * 1024 * 1024) {
                return NULL;
            }
            unsigned char *bytes = malloc(data.length);
            if (bytes != NULL) {
                memcpy(bytes, data.bytes, data.length);
                *length = data.length;
            }
            return bytes;
        } @catch (NSException *exception) {
            return NULL;
        }
    }
}

void messages_free_contacts(unsigned char *bytes) {
    free(bytes);
}
