#!/usr/bin/python
# -*- coding: utf-8 -*-

#
# Copyright (C) 2017  Martin Basti <mbasti@redhat.com>
#
# This program is free software: you can redistribute it and/or modify
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

from __future__ import print_function
import argparse
import time
from requests.exceptions import ConnectionError
from libpagure import Pagure

PAGURE_REPO = "freeipa"
MESSAGE = "{move_from} has been released, moving to {move_to} milestone"


def opened_tickets(pagure, milestone):
    tickets = pagure.list_issues(status="Open", milestones=[milestone])
    tickets_id = [(t['id'], t['title']) for t in tickets]
    return tickets_id


def move_tickets_to_milestone(pagure, move_from, move_to, message=None):
    """
    Move all opened tickets from one milestone to another
    :param pagure: Pagure instance
    :param move_from: source milestone
    :param move_to: destination milestone
    :param message: message to be added as comment to migrated issues
    """
    for t_num, t_title in opened_tickets(pagure, move_from):
        print("Moving ticket '#{}: {}' to milestone '{}'".format(
            t_num, t_title, move_to
        ))
        try:
            moved = False
            while not moved:
                try:
                    pagure.change_issue_milestone(t_num, move_to)
                except ConnectionError:
                    time.sleep(1)
                    continue
                else: moved = True
        except AttributeError:
            # Workaround https://pagure.io/libpagure/issue/23
            issue = pagure.issue_info(t_num)
            if issue['milestone'] != move_to:
                raise RuntimeError("Failed to move ticket to milestone")
        if message:
            pagure.comment_issue(t_num, message)
        time.sleep(1)  # Do not hammer pagure API

def parse_args():
    desc = """
    Migrate opened tickets to a new milestone\n \n

    Example: ./move-milestone "FreeIPA 4.5.1" "FreeIPA 4.5.2" --add-comment
    """
    # create the top-level parser
    parser = argparse.ArgumentParser(
        prog='move-milestone',
        description=desc)

    parser.add_argument('move_from', help='Migrate from this mileston')
    parser.add_argument('move_to', help='Migrate to this milestone')
    parser.add_argument('--message', dest='message',
                        help='Comment to be added to tickets, see '
                             '--add-comment option (allowed substitution '
                             '{move_from}, {move_to}). Default: "%s"'
                             % MESSAGE,
                        default=MESSAGE)
    parser.add_argument('--add-comment', dest='add_comment',
                        action="store_true",
                        help='Add comment to tickets about migrating',
                        default=False)
    parser.add_argument('--token', dest='token', action='store',
                        help='Pagure token for accessing issues',
                        metavar='TOKEN', default=None)
    parser.add_argument('--token-file', dest='token_file', action='store',
                        help='Path to file where pagure token is stored',
                        metavar='PATH', default=None)
    args = parser.parse_args()

    if not (args.token or args.token_file):
        raise RuntimeError(
            "Please specify --token or --token-file for pagure access")
    return args


def main():
    args = parse_args()

    pagure_token = None
    if args.token:
        pagure_token = args.token
    elif args.token_file:
        with open(args.token_file, 'r') as f:
            pagure_token = f.read().strip()
    else:
        RuntimeError("Please specify Pagure token")

    message = None
    if args.add_comment:
        message = args.message.format(
            move_from=args.move_from, move_to=args.move_to)

    pagure = Pagure(
        pagure_repository=PAGURE_REPO,
        pagure_token=pagure_token
    )

    move_tickets_to_milestone(
        pagure, args.move_from, args.move_to, message=message)

if __name__ == '__main__':
    main()
