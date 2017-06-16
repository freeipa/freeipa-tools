# Release engineering ansible playbooks

**Only 4.5+ build system is currently supported!**

## Configuration

Edit `group_vals/all/vars.yml` as necessary.

## Usage

### Update zanata

```bash
ansible-playbook -i hosts zanata_push.yml -e git_branch=ipa-4-5
```

### Release FreeIPA

This is currently incomplete and covers only:

- zanata push
- zanata pull
- updating contributors

The rest of the action will be automated in the future.
See https://www.freeipa.org/page/Release for the full guide.

```bash
ansible-playbook -i hosts release.yml -e git_branch=ipa-4-5
```

If it passes, you can push the changes to upstream repo.
