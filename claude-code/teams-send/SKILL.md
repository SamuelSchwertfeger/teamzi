---
name: teams-send
description: Send a Microsoft Teams chat message through the Teams app on this PC, by contact name. Use when the user says "send elliot: ...", "tell elliot ...", "message elliot ..." or similar.
---

# teams-send

Sends one Teams chat message through the Teams desktop app on this PC with `teamzi send`. Nothing goes through Microsoft Graph or any account connection: Teams opens the chat with the text typed in, and Teamzi presses Enter.

Edit this line after install:

EXE = `%USERPROFILE%/Desktop/CC/projects/teamzi/target/x86_64-pc-windows-gnu/release/teamzi.exe`

Contacts: `%APPDATA%/teamzi/contacts.txt`, one per line, `name = email`. Lines starting with `#` are comments.

## Steps

1. Split the request into a recipient name and the message text. Keep the text exactly as given; don't rewrite, fix or add to it.
2. Read the contacts file (if missing, treat as empty). Match the name case-insensitively against each contact's name: an exact full-name match wins, otherwise every contact whose name contains each word given.
   - One match: use it.
   - Several: ask which, listing them by full name ("Elliot Sam or Elliot Bob?"). Wait for the answer.
   - None: ask for their email address. After a successful send, offer to save it as `name = email`.
3. Show exactly what will be sent and wait for a yes:
   `To: Elliot Sam <esam@corp.com>` / `Message: running 5 late`
   Anything other than a clear yes means don't send.
4. Run, with the message in single quotes (write each `'` in the text as `'\''`):
   `"<EXE>" send <email> '<message>'`
5. Report the result in one line: `Sent to Elliot Sam: running 5 late`, or the error after `not sent:` verbatim. Never retry automatically: a retry could send the message twice.

## Limits

- Windows only. The PC must be on and unlocked with Teams signed in; Teamzi running keeps it awake.
- One message per command. Don't send to more than one person from a single request without asking per person.
- On Linux the output starts with `draft opened`: Enter is not pressed there, so report "Draft open in Teams; press Enter to send" instead of "Sent".
- Exit code 0 means Enter was pressed with Teams in front; it can't see whether Teams delivered the message. If the user reports a draft left unsent, tell them to look at Teams.
