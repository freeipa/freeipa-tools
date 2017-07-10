# Release engineering ansible playbooks

**Only 4.5+ build system and PATCH releases are currently supported!**

## Configuration

0. Install dependencies (not documented, please contribute a list when you
   find out all that's required)
1. Create a file containing a pagure token.
2. Edit `group_vals/all/vars.yml` as necessary.

## Usage

### Update zanata

```bash
ansible-playbook zanata_push.yml -e git_branch=ipa-4-5
```

### Release FreeIPA

This playbook can be used to do a patch release (4.5.x, ...) of FreeIPA.
Please note, major and minor version releases require some additional
manual steps, see https://www.freeipa.org/page/Release for the full guide.

The playbook produces artifacts (release notes, tarball, ...) that require
additional manual actions. You can find these artifacts in the 
`artifacts_location` directory, as configured above.

After the playbook finishes successfully, you still need to:

- upload the tarball to pagure
- create new milestone in pagure
- move tickets to new milestone
- update the wiki pages
- send the release notes e-mail

Not yet finished (WIP):

- PyPI release - currently only release to TestPyPI

#### Example usage

Release version 4.5.2.

```bash
ansible-playbook release.yml \
```
