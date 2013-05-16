#!/usr/bin/python
# Authors:
#   Martin Kosek <mkosek@redhat.com>
#
# Copyright (C) 2012  Red Hat
# see file 'COPYING' for use and warranty information
#
# This program is free software; you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <http://www.gnu.org/licenses/>.

import subprocess
import re
import os
import sys
from os.path import abspath
from optparse import OptionParser

# check current working dir
CLEAN_REPO_PATH     = "/home/mkosek/freeipa-clean"
TRAC_TICKET_PATH    = "https://fedorahosted.org/freeipa/ticket/"
TRAC_COMMIT_PATH    = "https://fedorahosted.org/freeipa/changeset/"
DEBUG               = False
WEB_BROWSER         = "google-chrome"

ALL_BRANCHES = ['master', 'ipa-3-2', 'ipa-3-1', 'ipa-3-0', 'ipa-2-2', 'ipa-2-1']
TARGET_BRANCHES = ['master', 'ipa-3-2']

def print_debug(obj):
    if DEBUG:
        print obj

def print_debug_cmd(cmd):
    print_debug("CMD: " + ' '.join(cmd))

def prepare_patch(patch):
    print_debug("Prepare patch %s" % patch)

    # Remove heading > sign as it crashes `git am PATCH`
    with open(patch, 'r+') as f:
        first_line = f.readline()

        if first_line[0] == '>':
            print "Removing extra heading '>' in", patch
            f.seek(0)
            new_first_line = first_line[1:-1] + " \n"
            f.write(new_first_line)

def git_checkout(branch):
    cmd = [ 'git', 'checkout', branch ]
    print_debug_cmd(cmd)
    subprocess.check_output(cmd, stderr=subprocess.STDOUT)

    cmd = [ 'git', 'status' ]
    print_debug_cmd(cmd)
    output = subprocess.check_output(cmd, stderr=subprocess.STDOUT)

    if output.find('branch is ahead') != -1:
        raise Exception('Branch %s is not clean!' % branch)
        pass

def git_fetch():
    cmd = [ 'git', 'fetch' ]
    print_debug_cmd(cmd)
    subprocess.check_output(cmd, stderr=subprocess.STDOUT)

def git_current_branch():
    cmd = [ 'git', 'status' ]
    print_debug_cmd(cmd)
    output = subprocess.check_output(cmd, stderr=subprocess.STDOUT)

    m = re.match(r'# On branch (\S+)', output)

    if m is None:
        raise Exception("Error when matching `git status` output: " + output)

    return m.group(1)

def git_rebase(target_branch):
    branch = git_current_branch()

    if branch != target_branch:
        raise Exception("Target branch and current branch differs!")

    # rebase
    cmd = [ 'git', 'rebase', 'origin/%s' % target_branch ]
    print_debug_cmd(cmd)
    subprocess.check_output(cmd, stderr=subprocess.STDOUT)

def git_last_commit():
    cmd = [ 'git', 'log', '-1', '--oneline' ]
    print_debug_cmd(cmd)
    output = subprocess.check_output(cmd, stderr=subprocess.STDOUT)

    m = re.match(r'(\S+) (.+)', output)

    if m is None:
        raise Exception("Error when matching `%s` output: %s" % (' '.join(cmd), output))

    last_commit_id = m.group(1)
    last_commit_name = m.group(2)

    return (last_commit_id, last_commit_name)

def git_last_commit_id():
    cmd = [ 'git', 'log', '-1' ]
    print_debug_cmd(cmd)
    output = subprocess.check_output(cmd, stderr=subprocess.STDOUT)

    m = re.match(r'commit\s+(\S+)', output)

    if m is None:
        raise Exception("Error when matching `%s` output: %s" % (' '.join(cmd), output))

    return m.group(1)

def git_am(patch):

    cmd = [ 'git', 'am', '-3', patch ]
    print_debug_cmd(cmd)

    try:
        output = subprocess.check_output(cmd, stderr=subprocess.STDOUT)
        pass
    except Exception, e:
        try:
            cmd = [ 'git', 'am', '--abort' ]
            print_debug_cmd(cmd)
            output = subprocess.check_output(cmd, stderr=subprocess.STDOUT)
        except:
            pass
        raise e

def git_cherry_pick(commit_id):
    last_commit_id,last_commit_name = git_last_commit()
    if commit_id == last_commit_id:
        print "git cherry-pick: Patch already commited"
        return

    cmd = [ 'git', 'cherry-pick', commit_id ]
    print_debug_cmd(cmd)
    subprocess.check_output(cmd, stderr=subprocess.STDOUT)

def git_push(target_branch, commit_id, last_commit_id, unattended=False):
    branch = git_current_branch()

    if branch != target_branch:
        raise Exception("Target branch and current branch differ!")

    cmd = [ 'git', 'push', 'origin', branch, '-n' ]
    print_debug_cmd(cmd)
    output = subprocess.check_output(cmd, stderr=subprocess.STDOUT)

    print_debug("git push -n output:\n%s" % output)
    m = re.search(r'\s(\S+)\.\.(\S+)\s+(\S+)\s+->\s+(\S+)', output)
    if m is None:
        raise Exception("Error when matching `%s` output: %s" % (' '.join(cmd), output))

    from_id = m.group(1)
    to_id = m.group(2)
    from_branch = m.group(3)
    to_branch = m.group(4)

    if from_id != last_commit_id:
        print from_id, last_commit_id
        raise Exception("git push check failed: trying to push more commits?")

    if to_id != commit_id:
        raise Exception("git push check failed: wrong target commit id")

    if from_branch != to_branch:
        raise Exception("git push check failed: source and target branch differ!")

    if unattended:
        print "[UNATTENDED] Pushing patch to branch", target_branch
    else:
        approval = None
        while approval != 'y' and approval != 'n' and approval != '':
            approval = raw_input("Push to branch %s [y]: " % target_branch)

        if approval == 'n':
            print "Push to branch %s skipped." % target_branch
            return

    cmd = [ 'git', 'push', 'origin', branch ]
    print_debug_cmd(cmd)
    subprocess.check_output(cmd, stderr=subprocess.STDOUT)

def get_tickets_from_patch(patch):
    tickets = []
    with open(patch, 'r') as f:
        for line in f:
            if line.startswith(u'---'): # the actual patch starts
                break

            m = re.match(r'(\S+\s)?%s(\d+)' % TRAC_TICKET_PATH, line)
            if m is not None:
                ticket = m.group(2)
                tickets.append(int(ticket))

            m = re.match(r'ticket\s+#?(\d+)\s+$', line)
            if m is not None:
                ticket = m.group(1)
                tickets.append(int(ticket))

            m = re.match(r'Subject:.*ticket\s+#?(\d+)\s+', line)
            if m is not None:
                ticket = m.group(1)
                tickets.append(int(ticket))

    return tickets

def get_ticket_links(patch):
    tickets = get_tickets_from_patch(patch)
    links = [TRAC_TICKET_PATH + str(ticket) for ticket in tickets]

    return links

def main():
    # parse arguments
    usage = "usage: %prog [options] PATCH"
    parser = OptionParser(usage=usage)
    parser.add_option("-v", action="store_true", dest="verbose",
            help="print verbose information")
    parser.add_option("--unattended", "-U", action="store_true",
            help="never prompt the user, presume positive answers" )
    parser.add_option("-b", "--branch", help="push only to the specified branch")

    (options, args) = parser.parse_args()

    if options.branch is not None and \
            any(branch not in ALL_BRANCHES for branch in options.branch.split(',')):
        parser.error("Target branch list (%s) does not follow known branches: %s" \
                % (options.branch, ', '.join(ALL_BRANCHES)))

    if not len(args):
        parser.error("No patch passed!")

    if options.verbose:
        DEBUG = True

    patches = []
    for patch in args:
        patches.append(abspath(patch))

    # change current working directory
    os.chdir(CLEAN_REPO_PATH)

    main_branch = True
    main_commit_id = None

    current_branch = git_current_branch()
    commit_ids = {}
    repo_changed = False

    # prepare patch for pushing
    for patch in patches:
        prepare_patch(patch)

    if options.branch is not None:
        target_branches = options.branch.split(",")
    else:
        target_branches = TARGET_BRANCHES

    patch = patches[0]  # TODO
    for branch in target_branches:
        git_checkout(branch)

        if main_branch:
            git_fetch()

        git_rebase(branch)
        last_stable_commit = git_last_commit()

        if main_branch:
            git_am(patch)
            main_commit_id,commit_name = git_last_commit()
            commit_id = main_commit_id
        else:
            if options.unattended:
                print "[UNATTENDED] Applying patch to branch", branch
            else:
                approval = None
                while approval != 'y' and approval != 'n' and approval != '':
                    approval = raw_input("Apply patch to branch %s [y]: " % branch)

                if approval == 'n':
                    continue

            git_cherry_pick(main_commit_id)
            commit_id,commit_name = git_last_commit()

        # do the actual push
        git_push(branch, commit_id, last_stable_commit[0], options.unattended)

        # save the commit ID
        repo_changed = True
        commit_ids[branch] = git_last_commit_id()

        main_branch = False

    # checkout to the original branch
    git_checkout(current_branch)

    if not repo_changed:
        print "-- RESULT ------------------------------"
        print "No changes"
        exit(0)

    # print commit IDs
    print "-- TRAC --------------------------------"
    commit_info = []
    for branch in target_branches:
        if branch in commit_ids:
            commit_info.append("%s: %s" % (branch, commit_ids[branch]))
    print "[[BR]]\n".join(commit_info)

    print "-- BUGZILLA ----------------------------"
    print "Fixed upstream:"
    commit_info = []
    for branch in target_branches:
        if branch in commit_ids:
            commit_info.append("%s: %s%s" % (branch, TRAC_COMMIT_PATH, commit_ids[branch]))
    print "\n".join(commit_info)

    links = get_ticket_links(patch)
    if links:
        print "-- ATTACHED LINKS ----------------------"
        print "\n".join(links)

        approval = None
        if options.unattended:
            approval = 'y'
        else:
            opts = ['y', 'n', '']
            while approval not in opts:
                approval = raw_input("Open %d Trac links attached to the patch? [y]: " % len(links))

        if approval in ['y', '']:
            with open(os.devnull, 'w') as fnull:
                for link in links:
                    # avoid chrome annoying errors printed out by webbrowser.open()
                    subprocess.Popen([WEB_BROWSER, link], stdout=fnull, stderr=fnull)

if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit("\nProgram interrupted")

